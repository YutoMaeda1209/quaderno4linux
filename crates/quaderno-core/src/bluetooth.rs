//! Bluetooth 接続 (Classic Bluetooth PAN)。
//!
//! Quaderno は NAP サービス (`0x1116`) を提供し、PC は PANU として接続して BNEP 上で IP 通信する。
//! ペアリング (SSP の数値比較) は端末の Bluetooth ペアリング画面を開いている間だけ可能で、
//! デスクトップの Bluetooth 設定から行う。端末はこのリンク上で DHCP に応答しないので、PC 側には
//! 固定の IPv4 アドレスが必要になる。端末のアドレスは `172.25.48.1` だった。
//!
//! ペアリング済みの端末は BlueZ の `bluetoothctl` で列挙し、PAN リンクは手動アドレスを使う専用の
//! 接続プロファイルで NetworkManager (`nmcli`) から確立する。

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::address::DeviceAddress;
use crate::client::device_information;
use crate::discovery::{discover, Transport};
use crate::error::{Error, Result};

/// Bluetooth NAP (Network Access Point) サービスクラスの UUID。
pub const NAP_UUID: &str = "00001116-0000-1000-8000-00805f9b34fb";
/// PAN リンク上で端末が使っていたアドレス。
pub const DEFAULT_DEVICE_IP: Ipv4Addr = Ipv4Addr::new(172, 25, 48, 1);
/// PAN リンクの PC 側に割り当てる固定アドレス (プレフィックス付き)。
pub const HOST_ADDRESS: &str = "172.25.48.2/24";
/// BNEP リンクは接続直後に不安定なので、この回数だけ再試行する。
const CONNECT_ATTEMPTS: usize = 3;

/// NAP サービスを提供するペアリング済みの Bluetooth 端末。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BluetoothDevice {
    /// BD アドレス (例: `AA:BB:CC:DD:EE:2E`)。
    pub address: String,
    /// 名前 (例: `FMVDP51_1234567`)。
    pub name: String,
    pub connected: bool,
}

impl BluetoothDevice {
    /// 名前が Quaderno らしいか (`FMVDP51_<serial>` など)。
    pub fn is_quaderno(&self) -> bool {
        self.name.starts_with("FMVDP")
    }

    /// このアプリが端末用に管理する NetworkManager のプロファイル名。
    pub fn connection_name(&self) -> String {
        format!("Quaderno Bluetooth {}", self.address)
    }
}

/// ネットワークインターフェースが Bluetooth (BNEP) のものか。
pub fn is_bluetooth_interface(name: &str) -> bool {
    if name.starts_with("bnep") {
        return true;
    }
    std::fs::read_to_string(format!("/sys/class/net/{name}/uevent"))
        .map(|u| u.lines().any(|l| l.trim() == "DEVTYPE=bluetooth"))
        .unwrap_or(false)
}

fn run(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Bluetooth(format!("{program} が見つかりません。{}", install_hint(program)))
            } else {
                Error::Bluetooth(format!("{program} を実行できません: {e}"))
            }
        })?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Bluetooth(format!("{program} {} がタイムアウトしました", args.join(" "))));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out)?;
    }
    if let Some(mut s) = child.stderr.take() {
        s.read_to_string(&mut err)?;
    }
    if !status.success() {
        let msg = if err.trim().is_empty() { out.trim() } else { err.trim() };
        return Err(Error::Bluetooth(format!("{program} {}: {msg}", args.first().copied().unwrap_or(""))));
    }
    Ok(out)
}

fn install_hint(program: &str) -> &'static str {
    match program {
        "bluetoothctl" => "BlueZ (Ubuntu: bluez / Fedora: bluez) をインストールしてください",
        "nmcli" => "NetworkManager が必要です",
        _ => "",
    }
}

/// `bluetoothctl devices` の出力 (`Device AA:BB:.. Name`) を解析する。
fn parse_device_list(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Device ")?;
            let (addr, name) = rest.split_once(' ').unwrap_or((rest, ""));
            (addr.len() == 17).then(|| (addr.to_uppercase(), name.trim().to_string()))
        })
        .collect()
}

/// `bluetoothctl info` の解析結果。
#[derive(Debug, Default, PartialEq)]
struct Info {
    name: Option<String>,
    paired: bool,
    connected: bool,
    nap: bool,
}

fn parse_info(out: &str) -> Info {
    let mut info = Info::default();
    for line in out.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Name:") {
            info.name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Paired:") {
            info.paired = v.trim() == "yes";
        } else if let Some(v) = line.strip_prefix("Connected:") {
            info.connected = v.trim() == "yes";
        } else if line.starts_with("UUID:") && line.to_lowercase().contains(NAP_UUID) {
            info.nap = true;
        }
    }
    info
}

/// NAP サービスを提供するペアリング済みの端末を、Quaderno を先にして列挙する。
pub fn paired_devices() -> Result<Vec<BluetoothDevice>> {
    let t = Duration::from_secs(10);
    // `devices Paired` は BlueZ 5.65 以降が必要。それより古いバージョンでは `paired-devices` を使う
    // 。
    let list = match run("bluetoothctl", &["devices", "Paired"], t) {
        Ok(out) if !out.trim().is_empty() => out,
        _ => run("bluetoothctl", &["paired-devices"], t).unwrap_or_default(),
    };
    let mut devices = Vec::new();
    for (address, listed_name) in parse_device_list(&list) {
        let info = parse_info(&run("bluetoothctl", &["info", &address], t)?);
        if !info.nap || !info.paired {
            continue;
        }
        devices.push(BluetoothDevice { name: info.name.unwrap_or(listed_name), address, connected: info.connected });
    }
    devices.sort_by_key(|d| (!d.is_quaderno(), d.name.clone()));
    Ok(devices)
}

/// NetworkManager の PANU プロファイルを作成・更新するための引数。
fn profile_settings(dev: &BluetoothDevice) -> Vec<String> {
    let mut args: Vec<String> = [
        "bluetooth.bdaddr",
        &dev.address,
        "bluetooth.type",
        "panu",
        // 端末は PAN リンク上で DHCP に応答しない。
        "ipv4.method",
        "manual",
        "ipv4.addresses",
        HOST_ADDRESS,
        "ipv4.never-default",
        "yes",
        "ipv6.method",
        "link-local",
        "connection.autoconnect",
        "no",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // 現在のユーザーが所有するプロファイルは管理者権限なしで管理できる。
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() && user != "root" {
            args.extend(["connection.permissions".to_string(), format!("user:{user}")]);
        }
    }
    args
}

/// `dev` との PAN リンクを確立し、ネットワークインターフェース名を返す。
pub fn connect_pan(dev: &BluetoothDevice) -> Result<String> {
    let name = dev.connection_name();
    let t = Duration::from_secs(20);
    let existing = run("nmcli", &["-t", "-f", "NAME", "connection", "show"], t)?;
    let mut args = if existing.lines().any(|l| l == name) {
        vec!["connection", "modify", name.as_str()]
    } else {
        vec!["connection", "add", "type", "bluetooth", "con-name", name.as_str()]
    };
    let settings = profile_settings(dev);
    args.extend(settings.iter().map(String::as_str));
    run("nmcli", &args, t)?;

    let mut attempt = 1;
    while let Err(e) = run("nmcli", &["--wait", "45", "connection", "up", name.as_str()], Duration::from_secs(60)) {
        log::warn!("Bluetooth PAN 接続に失敗しました ({attempt}/{CONNECT_ATTEMPTS}): {e}");
        if attempt == CONNECT_ATTEMPTS {
            return Err(Error::Bluetooth(format!(
                "Bluetooth PAN に接続できません。端末の Bluetooth が有効か確認してください ({e})"
            )));
        }
        attempt += 1;
        std::thread::sleep(Duration::from_secs(2));
    }

    let iface = run("nmcli", &["-g", "GENERAL.IP-IFACE", "connection", "show", name.as_str()], t)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !iface.is_empty() {
        return Ok(iface);
    }
    bluetooth_interfaces()
        .into_iter()
        .next()
        .ok_or_else(|| Error::Bluetooth("Bluetooth のネットワークインターフェースが見つかりません".into()))
}

/// PAN リンクを切断する (エラーは無視する)。
pub fn disconnect_pan(dev: &BluetoothDevice) {
    let _ = run("nmcli", &["connection", "down", &dev.connection_name()], Duration::from_secs(20));
}

/// 現在存在する Bluetooth のネットワークインターフェース。
pub fn bluetooth_interfaces() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir("/sys/class/net")
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    out.retain(|n| is_bluetooth_interface(n));
    out.sort();
    out
}

/// 確立した PAN リンク上で端末を探す。まず mDNS (PAN 上でも使える) を試し、次に端末が使っていたアド
/// レスを試す。
pub fn locate_device(iface: &str) -> Result<DeviceAddress> {
    for _ in 0..3 {
        let found = discover(Duration::from_millis(2000));
        if let Some(d) = found
            .iter()
            .filter(|d| d.transport == Transport::Bluetooth)
            .find(|d| d.interface.as_deref() == Some(iface) && d.info.is_some())
        {
            return Ok(d.address);
        }
        let fallback = DeviceAddress::new(IpAddr::V4(DEFAULT_DEVICE_IP), 0);
        if device_information(&fallback, Duration::from_secs(3)).is_ok() {
            return Ok(fallback);
        }
    }
    Err(Error::Bluetooth(format!("{iface} 上で端末が応答しません")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_list() {
        let out = "Device AA:BB:CC:DD:EE:2E FMVDP51_1234567\nDevice 11:22:33:44:55:66 My Phone\nnoise\n";
        assert_eq!(
            parse_device_list(out),
            vec![
                ("AA:BB:CC:DD:EE:2E".into(), "FMVDP51_1234567".into()),
                ("11:22:33:44:55:66".into(), "My Phone".into())
            ]
        );
    }

    #[test]
    fn parses_info() {
        let out = "Device AA:BB:CC:DD:EE:2E (public)\n\tName: FMVDP51_1234567\n\tClass: 0x0002020c\n\
                   \tPaired: yes\n\tConnected: no\n\
                   \tUUID: NAP                       (00001116-0000-1000-8000-00805f9b34fb)\n\
                   \tUUID: PnP Information           (00001200-0000-1000-8000-00805f9b34fb)\n";
        assert_eq!(
            parse_info(out),
            Info { name: Some("FMVDP51_1234567".into()), paired: true, connected: false, nap: true }
        );
        assert!(!parse_info("\tName: Headset\n\tPaired: yes\n").nap);
    }

    #[test]
    fn profile_uses_static_ipv4_panu() {
        let dev = BluetoothDevice { address: "AA:BB:CC:DD:EE:2E".into(), name: "FMVDP51_1".into(), connected: false };
        let s = profile_settings(&dev);
        let get = |k: &str| s.iter().position(|x| x == k).map(|i| s[i + 1].as_str());
        assert_eq!(get("bluetooth.type"), Some("panu"));
        assert_eq!(get("ipv4.method"), Some("manual"));
        assert_eq!(get("ipv4.addresses"), Some(HOST_ADDRESS));
        assert!(dev.is_quaderno());
    }
}

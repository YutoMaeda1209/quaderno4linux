//! Bluetooth connection support (Classic Bluetooth PAN).
//!
//! The Quaderno offers the NAP service (`0x1116`); the PC connects as PANU and
//! IP runs over BNEP. Pairing (SSP numeric comparison) only works while the
//! device's Bluetooth pairing screen is open and is done in the desktop's
//! Bluetooth settings. The device does not answer DHCP on this link, so the PC
//! side needs a static IPv4 address; the device was observed at `172.25.48.1`.
//!
//! Paired devices are listed with BlueZ's `bluetoothctl`, and the PAN link is
//! brought up through NetworkManager (`nmcli`) with a dedicated connection
//! profile that uses a manual address.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::address::DeviceAddress;
use crate::client::device_information;
use crate::discovery::{discover, Transport};
use crate::error::{Error, Result};

/// Bluetooth NAP (Network Access Point) service class UUID.
pub const NAP_UUID: &str = "00001116-0000-1000-8000-00805f9b34fb";
/// Address the device was observed to use on the PAN link.
pub const DEFAULT_DEVICE_IP: Ipv4Addr = Ipv4Addr::new(172, 25, 48, 1);
/// Static address (with prefix) given to the PC side of the PAN link.
pub const HOST_ADDRESS: &str = "172.25.48.2/24";
/// BNEP links are flaky right after connecting; retry this many times.
const CONNECT_ATTEMPTS: usize = 3;

/// A paired Bluetooth device that offers the NAP service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BluetoothDevice {
    /// BD address, e.g. `AA:BB:CC:DD:EE:2E`.
    pub address: String,
    /// Name, e.g. `FMVDP51_1234567`.
    pub name: String,
    pub connected: bool,
}

impl BluetoothDevice {
    /// Whether the name looks like a Quaderno (`FMVDP51_<serial>`, ...).
    pub fn is_quaderno(&self) -> bool {
        self.name.starts_with("FMVDP")
    }

    /// Name of the NetworkManager profile this app manages for the device.
    pub fn connection_name(&self) -> String {
        format!("Quaderno Bluetooth {}", self.address)
    }
}

/// Whether a network interface is a Bluetooth (BNEP) interface.
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

/// Parses `bluetoothctl devices` output (`Device AA:BB:.. Name`).
fn parse_device_list(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Device ")?;
            let (addr, name) = rest.split_once(' ').unwrap_or((rest, ""));
            (addr.len() == 17).then(|| (addr.to_uppercase(), name.trim().to_string()))
        })
        .collect()
}

/// Parsed `bluetoothctl info` output.
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

/// Lists paired devices that offer the NAP service, Quaderno devices first.
pub fn paired_devices() -> Result<Vec<BluetoothDevice>> {
    let t = Duration::from_secs(10);
    // `devices Paired` needs BlueZ >= 5.65; older versions have `paired-devices`.
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

/// Arguments for creating/updating the NetworkManager PANU profile.
fn profile_settings(dev: &BluetoothDevice) -> Vec<String> {
    let mut args: Vec<String> = [
        "bluetooth.bdaddr",
        &dev.address,
        "bluetooth.type",
        "panu",
        // The device does not answer DHCP on the PAN link.
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
    // A profile owned by the current user can be managed without admin rights.
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() && user != "root" {
            args.extend(["connection.permissions".to_string(), format!("user:{user}")]);
        }
    }
    args
}

/// Brings up the PAN link to `dev` and returns the network interface name.
pub fn connect_pan(dev: &BluetoothDevice) -> Result<String> {
    let name = dev.connection_name();
    let t = Duration::from_secs(20);
    let existing = run("nmcli", &["-t", "-f", "NAME", "connection", "show"], t)?;
    let settings = profile_settings(dev);
    let settings: Vec<&str> = settings.iter().map(String::as_str).collect();
    if existing.lines().any(|l| l == name) {
        let mut args = vec!["connection", "modify", name.as_str()];
        args.extend(&settings);
        run("nmcli", &args, t)?;
    } else {
        let mut args = vec!["connection", "add", "type", "bluetooth", "con-name", name.as_str()];
        args.extend(&settings);
        run("nmcli", &args, t)?;
    }

    let mut last_error = None;
    for attempt in 1..=CONNECT_ATTEMPTS {
        match run("nmcli", &["--wait", "45", "connection", "up", name.as_str()], Duration::from_secs(60)) {
            Ok(_) => {
                last_error = None;
                break;
            }
            Err(e) => {
                log::warn!("Bluetooth PAN 接続に失敗しました ({attempt}/{CONNECT_ATTEMPTS}): {e}");
                last_error = Some(e);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    if let Some(e) = last_error {
        return Err(Error::Bluetooth(format!(
            "Bluetooth PAN に接続できません。端末の Bluetooth が有効か確認してください ({e})"
        )));
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

/// Takes the PAN link down (errors are ignored).
pub fn disconnect_pan(dev: &BluetoothDevice) {
    let _ = run("nmcli", &["connection", "down", &dev.connection_name()], Duration::from_secs(20));
}

/// Bluetooth network interfaces currently present.
pub fn bluetooth_interfaces() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir("/sys/class/net")
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    out.retain(|n| is_bluetooth_interface(n));
    out.sort();
    out
}

/// Finds the device on an established PAN link: mDNS first (it works over
/// PAN), then the address the device was observed to use.
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

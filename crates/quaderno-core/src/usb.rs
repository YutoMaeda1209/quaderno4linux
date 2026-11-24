//! USB 接続。
//!
//! Quaderno は接続すると USB シリアル (CDC-ACM) デバイス (`04c5:1656`) として認識される。
//! そのシリアルポートに 10 バイトのコマンドを書き込むと、USB Ethernet モード (`04c5:1657`、
//! RNDIS または CDC-ECM) に切り替わる。以降は新しいネットワークインターフェース上の
//! IPv6 リンクローカルアドレスで端末に到達する。

use std::fs;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nix::sys::termios::{self, SetArg};

use crate::error::{Error, Result};

pub const VENDOR_ID: &str = "04c5";
/// シリアル (初期) モードでのプロダクト ID。
pub const PRODUCT_ID_SERIAL: &str = "1656";
/// USB Ethernet モードでのプロダクト ID。
pub const PRODUCT_ID_ETHERNET: &str = "1657";

/// ログイン中のユーザーにシリアルポートへのアクセスを許可する udev ルール。
pub const UDEV_RULE: &str = include_str!("../../../packaging/linux/70-quaderno.rules");
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-quaderno.rules";
/// deb / rpm パッケージがルールをインストールする場所。
pub const PACKAGED_UDEV_RULE_PATH: &str = "/usr/lib/udev/rules.d/70-quaderno.rules";

/// 切り替え先の USB Ethernet の方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbNetMode {
    /// RNDIS (Linux の `rndis_host` ドライバで動作する)。
    Rndis,
    /// CDC-ECM (`cdc_ether` ドライバ)。
    CdcEcm,
}

impl UsbNetMode {
    pub fn command(self) -> [u8; 10] {
        let mode = match self {
            UsbNetMode::Rndis => 0x00,
            UsbNetMode::CdcEcm => 0x01,
        };
        [0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, mode, 0x04]
    }
}

/// sysfs ノードに最も近い祖先の USB デバイスから `idVendor`/`idProduct` を読み取る。
fn usb_ids(sysfs_device: &Path) -> Option<(String, String)> {
    let mut dir = fs::canonicalize(sysfs_device).ok()?;
    for _ in 0..6 {
        let v = dir.join("idVendor");
        let p = dir.join("idProduct");
        if v.exists() && p.exists() {
            let v = fs::read_to_string(v).ok()?.trim().to_lowercase();
            let p = fs::read_to_string(p).ok()?.trim().to_lowercase();
            return Some((v, p));
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// シリアルモードの Quaderno のシリアルポート (`/dev/ttyACM*`)。
pub fn find_serial_ports() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/tty") else { return out };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("ttyACM") {
            continue;
        }
        if let Some((v, p)) = usb_ids(&entry.path().join("device")) {
            if v == VENDOR_ID && p == PRODUCT_ID_SERIAL {
                out.push(PathBuf::from("/dev").join(name));
            }
        }
    }
    out.sort();
    out
}

/// Quaderno が提供する USB Ethernet インターフェース。
#[derive(Clone, Debug)]
pub struct UsbInterface {
    pub name: String,
    pub index: u32,
    /// インターフェースが管理上有効 (up) なら `true`。
    pub up: bool,
}

/// USB Ethernet モードの Quaderno のネットワークインターフェース。
pub fn find_network_interfaces() -> Vec<UsbInterface> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else { return out };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((v, p)) = usb_ids(&entry.path().join("device")) else { continue };
        if v != VENDOR_ID || p != PRODUCT_ID_ETHERNET {
            continue;
        }
        let flags = fs::read_to_string(entry.path().join("flags"))
            .ok()
            .and_then(|f| u32::from_str_radix(f.trim().trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let index =
            fs::read_to_string(entry.path().join("ifindex")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        out.push(UsbInterface { name, index, up: flags & 0x1 != 0 });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// シリアルポートにモード切り替えのコマンドを送る。
pub fn switch_to_ethernet(port: &Path, mode: UsbNetMode) -> Result<()> {
    let mut file =
        fs::OpenOptions::new().read(true).write(true).custom_flags(nix::libc::O_NOCTTY).open(port).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::UsbPermission(port.to_path_buf())
            } else {
                Error::Usb(format!("{} を開けません: {e}", port.display()))
            }
        })?;
    // raw モード: エコーなし、改行の変換なし、8N1。
    let mut tio = termios::tcgetattr(file.as_fd()).map_err(|e| Error::Usb(e.to_string()))?;
    termios::cfmakeraw(&mut tio);
    termios::tcsetattr(file.as_fd(), SetArg::TCSANOW, &tio).map_err(|e| Error::Usb(e.to_string()))?;
    file.write_all(&mode.command())?;
    file.flush()?;
    termios::tcdrain(file.as_fd()).map_err(|e| Error::Usb(e.to_string()))?;
    Ok(())
}

/// 同梱の udev ルールが現在の内容でインストールされていれば `true`。
pub fn udev_rule_installed() -> bool {
    // /etc のルールは、deb / rpm パッケージが /usr/lib にインストールしたルールより優先される。
    fs::read_to_string(UDEV_RULE_PATH)
        .or_else(|_| fs::read_to_string(PACKAGED_UDEV_RULE_PATH))
        .is_ok_and(|s| s == UDEV_RULE)
}

/// 同梱の udev ルールを `pkexec` でインストールし (デスクトップの polkit エージェントが管理者パスワ
/// ードを尋ねる)、接続中のデバイスに適用し直す。
pub fn install_udev_rule() -> Result<()> {
    // ルールは標準入力で渡すので、root が読める一時ファイルは必要ない。
    let script = format!(
        "set -e; umask 022; cat > {UDEV_RULE_PATH}; udevadm control --reload; \
         if [ $# -gt 0 ]; then udevadm trigger --settle --action=change \"$@\"; fi"
    );
    let mut child = Command::new("pkexec")
        .args(["/bin/sh", "-c", &script, "sh"])
        .args(find_serial_ports())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Usb(format!("pkexec を実行できません: {e}")))?;
    child.stdin.take().expect("piped stdin").write_all(UDEV_RULE.as_bytes())?;
    let output = child.wait_with_output()?;
    match output.status.code() {
        Some(0) => Ok(()),
        // pkexec: 126 = ダイアログが閉じられた、127 = 権限がないか polkit エージェントがない。
        Some(126) => Err(Error::Usb("udev ルールのインストールがキャンセルされました".into())),
        Some(127) => Err(Error::Usb(
            "管理者権限を取得できませんでした (polkit エージェントが動作しているか確認してください)".into(),
        )),
        _ => Err(Error::Usb(format!(
            "udev ルールをインストールできませんでした: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

/// 接続中のデバイスの現在の USB の状態。
#[derive(Clone, Debug, Default)]
pub struct UsbState {
    pub serial_ports: Vec<PathBuf>,
    pub interfaces: Vec<UsbInterface>,
}

pub fn scan() -> UsbState {
    UsbState { serial_ports: find_serial_ports(), interfaces: find_network_interfaces() }
}

/// 接続中でシリアルモードのままの端末をすべて USB Ethernet に切り替え、ネットワークインターフェース
/// が現れるのを (最大 `timeout` まで) 待つ。
pub fn ensure_ethernet(mode: UsbNetMode, timeout: std::time::Duration) -> Result<Vec<UsbInterface>> {
    let state = scan();
    if state.serial_ports.is_empty() && state.interfaces.is_empty() {
        return Err(Error::Usb("USB 接続された Quaderno が見つかりません".into()));
    }
    for port in &state.serial_ports {
        log::info!("{} を USB Ethernet モードに切り替えます", port.display());
        switch_to_ethernet(port, mode)?;
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let ifaces = find_network_interfaces();
        if !ifaces.is_empty() && (state.serial_ports.is_empty() || ifaces.iter().any(|i| i.up)) {
            return Ok(ifaces);
        }
        if std::time::Instant::now() >= deadline {
            if !ifaces.is_empty() {
                return Ok(ifaces);
            }
            return Err(Error::Usb("USB ネットワークインターフェースが現れませんでした".into()));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(UsbNetMode::Rndis.command(), [1, 0, 0, 1, 0, 0, 0, 1, 0, 4]);
        assert_eq!(UsbNetMode::CdcEcm.command()[8], 1);
    }

    #[test]
    fn scan_does_not_panic() {
        let _ = scan();
    }
}

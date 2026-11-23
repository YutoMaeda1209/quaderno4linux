//! USB connection support.
//!
//! When plugged in, the Quaderno enumerates as a USB serial (CDC-ACM) device
//! (`04c5:1656`). Writing a 10-byte command to that serial port switches it to
//! USB Ethernet mode (`04c5:1657`, RNDIS or CDC-ECM). The device then has to be
//! reached through the IPv6 link-local address on the new network interface.

use std::fs;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::sys::termios::{self, SetArg};

use crate::error::{Error, Result};

pub const VENDOR_ID: &str = "04c5";
/// Product id while in serial (initial) mode.
pub const PRODUCT_ID_SERIAL: &str = "1656";
/// Product id while in USB Ethernet mode.
pub const PRODUCT_ID_ETHERNET: &str = "1657";

/// USB Ethernet flavour to switch to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbNetMode {
    /// RNDIS (works with the Linux `rndis_host` driver).
    Rndis,
    /// CDC-ECM (`cdc_ether` driver).
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

/// Reads `idVendor`/`idProduct` from the closest USB device ancestor of a sysfs node.
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

/// Serial ports (`/dev/ttyACM*`) that belong to a Quaderno in serial mode.
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

/// A USB Ethernet interface exposed by a Quaderno.
#[derive(Clone, Debug)]
pub struct UsbInterface {
    pub name: String,
    pub index: u32,
    /// `true` when the interface is administratively up.
    pub up: bool,
}

/// Network interfaces that belong to a Quaderno in USB Ethernet mode.
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

/// Sends the mode-switch command to a serial port.
pub fn switch_to_ethernet(port: &Path, mode: UsbNetMode) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOCTTY)
        .open(port)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::Usb(format!(
                    "{} を開く権限がありません。dialout グループに参加するか、同梱の udev ルールをインストールしてください",
                    port.display()
                ))
            } else {
                Error::Usb(format!("{} を開けません: {e}", port.display()))
            }
        })?;
    // Raw mode: no echo, no newline conversion, 8N1.
    let mut tio = termios::tcgetattr(file.as_fd()).map_err(|e| Error::Usb(e.to_string()))?;
    termios::cfmakeraw(&mut tio);
    termios::tcsetattr(file.as_fd(), SetArg::TCSANOW, &tio).map_err(|e| Error::Usb(e.to_string()))?;
    file.write_all(&mode.command())?;
    file.flush()?;
    termios::tcdrain(file.as_fd()).map_err(|e| Error::Usb(e.to_string()))?;
    Ok(())
}

/// Current USB state of attached devices.
#[derive(Clone, Debug, Default)]
pub struct UsbState {
    pub serial_ports: Vec<PathBuf>,
    pub interfaces: Vec<UsbInterface>,
}

pub fn scan() -> UsbState {
    UsbState { serial_ports: find_serial_ports(), interfaces: find_network_interfaces() }
}

/// Switches every attached device that is still in serial mode to USB Ethernet
/// and waits (up to `timeout`) for the network interface to appear.
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

//! 端末のアドレス指定。
//!
//! USB 経由の Quaderno には IPv6 リンクローカルアドレスでしか到達できず、`fe80::1%usb0` のような
//! ゾーン (スコープ) ID が必要になる。URL にはゾーン ID を確実には含められないため、HTTP クライアントは
//! 常に固定のプレースホルダーのホスト名に接続し、名前解決を実際のソケットアドレス (スコープ ID を含む) で
//! 上書きする。

use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6, ToSocketAddrs};

use crate::error::{Error, Result};

/// リクエストの URL に使うプレースホルダーのホスト名。
pub const DEVICE_HOST: &str = "quaderno.device";
/// 認証不要の HTTP ポート (登録、端末情報)。
pub const HTTP_PORT: u16 = 8080;
/// 認証が必要な HTTPS ポート (メインの API)。
pub const HTTPS_PORT: u16 = 8443;

/// 名前解決済みの端末アドレス。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceAddress {
    pub ip: IpAddr,
    /// IPv6 のスコープ (インターフェースのインデックス)。該当しない場合は 0。
    pub scope_id: u32,
}

impl DeviceAddress {
    pub fn new(ip: IpAddr, scope_id: u32) -> Self {
        Self { ip, scope_id }
    }

    /// `192.168.0.10`、`fe80::1%usb0`、`fe80::1%3`、`[fe80::1%usb0]`、または名前解決できるホスト名
    /// (`Android.local` など) を解析する。
    pub fn parse(input: &str) -> Result<Self> {
        let s = input.trim().trim_start_matches('[').trim_end_matches(']');
        if s.is_empty() {
            return Err(Error::InvalidAddress(input.into()));
        }
        if let Some((ip, zone)) = s.split_once('%') {
            let ip: Ipv6Addr = ip.parse().map_err(|_| Error::InvalidAddress(input.into()))?;
            let scope_id = match zone.parse::<u32>() {
                Ok(n) => n,
                Err(_) => nix::net::if_::if_nametoindex(zone)
                    .map_err(|_| Error::InvalidAddress(format!("インターフェース {zone} が見つかりません")))?,
            };
            return Ok(Self::new(IpAddr::V6(ip), scope_id));
        }
        if let Ok(ip) = s.parse::<IpAddr>() {
            return Ok(Self::new(ip, 0));
        }
        let resolved = (s, HTTP_PORT)
            .to_socket_addrs()
            .map_err(|e| Error::InvalidAddress(format!("{s}: {e}")))?
            .next()
            .ok_or_else(|| Error::InvalidAddress(s.into()))?;
        Ok(Self::from_socket_addr(resolved))
    }

    pub fn from_socket_addr(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(v4) => Self::new(IpAddr::V4(*v4.ip()), 0),
            SocketAddr::V6(v6) => Self::new(IpAddr::V6(*v6.ip()), v6.scope_id()),
        }
    }

    pub fn socket_addr(&self, port: u16) -> SocketAddr {
        match self.ip {
            IpAddr::V4(ip) => SocketAddr::new(IpAddr::V4(ip), port),
            IpAddr::V6(ip) => SocketAddr::V6(SocketAddrV6::new(ip, port, 0, self.scope_id)),
        }
    }

    /// スコープ付きアドレスのインターフェース名。
    pub fn interface_name(&self) -> Option<String> {
        if self.scope_id == 0 {
            return None;
        }
        nix::net::if_::if_nameindex()
            .ok()?
            .iter()
            .find(|i| i.index() == self.scope_id)
            .map(|i| i.name().to_string_lossy().into_owned())
    }
}

impl fmt::Display for DeviceAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.ip, self.scope_id) {
            (IpAddr::V6(ip), 0) => write!(f, "{ip}"),
            (IpAddr::V6(ip), scope) => match self.interface_name() {
                Some(name) => write!(f, "{ip}%{name}"),
                None => write!(f, "{ip}%{scope}"),
            },
            (IpAddr::V4(ip), _) => write!(f, "{ip}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        let a = DeviceAddress::parse("192.168.1.20").unwrap();
        assert_eq!(a.ip.to_string(), "192.168.1.20");
        let b = DeviceAddress::parse("[fe80::1%7]").unwrap();
        assert_eq!(b.scope_id, 7);
        let c = DeviceAddress::parse("fe80::1%lo").unwrap();
        assert!(c.scope_id > 0);
        assert_eq!(c.to_string(), "fe80::1%lo");
        assert!(DeviceAddress::parse("fe80::1%doesnotexist0").is_err());
        assert!(DeviceAddress::parse("").is_err());
    }
}

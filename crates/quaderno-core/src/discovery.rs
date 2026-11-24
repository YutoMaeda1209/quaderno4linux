//! mDNS / DNS-SD による端末の検索。
//!
//! Quaderno は `_dp_fujitsu._tcp.local.` を広告する (Sony DPT-RP1 は `_digitalpaper._tcp.local.`)。
//! すべてのインターフェースで一時ポートから「レガシーユニキャスト」のクエリを 1 回送る
//! (IPv4 は `224.0.0.251`、IPv6 は `ff02::fb%iface`)。応答側はこちらのソケットに直接応答するので、
//! ポート 5353 で待ち受ける (Avahi と衝突する) 必要がない。各応答の送信元アドレス (IPv6 のスコープを含む) が
//! 端末のアドレスになる。

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::address::{DeviceAddress, HTTP_PORT};
use crate::client::device_information;
use crate::model::DeviceInfo;

pub const SERVICE_TYPES: [&str; 3] = ["_dp_fujitsu._tcp.local", "_digitalpaper._tcp.local", "_dp_readmoo._tcp.local"];

const MDNS_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);
const MDNS_PORT: u16 = 5353;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;

/// 端末に到達した経路。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Transport {
    /// USB Ethernet (RNDIS / CDC-ECM)。
    Usb,
    /// Wi-Fi またはその他の LAN インターフェース。
    Network,
    /// Bluetooth PAN (BNEP)。
    Bluetooth,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::Network => "Wi-Fi / LAN",
            Transport::Bluetooth => "Bluetooth",
        }
    }

    fn of_interface(name: &str, usb_ifaces: &HashSet<String>) -> Self {
        if usb_ifaces.contains(name) {
            Transport::Usb
        } else if crate::bluetooth::is_bluetooth_interface(name) {
            Transport::Bluetooth
        } else {
            Transport::Network
        }
    }
}

/// ネットワーク上で見つかった端末。
#[derive(Clone, Debug)]
pub struct DiscoveredDevice {
    pub address: DeviceAddress,
    pub port: u16,
    pub interface: Option<String>,
    pub info: Option<DeviceInfo>,
    pub transport: Transport,
}

impl DiscoveredDevice {
    pub fn label(&self) -> String {
        let name = self.info.as_ref().map(|i| i.display_name()).unwrap_or_else(|| "Quaderno".into());
        format!("{name} – {} – {}", self.transport.label(), self.address)
    }
}

/// すべてのサービスタイプの PTR レコードを問い合わせる DNS クエリのパケットを組み立てる。
fn build_query() -> Vec<u8> {
    let mut p = Vec::with_capacity(128);
    p.extend_from_slice(&[0x00, 0x00]); // ID
    p.extend_from_slice(&[0x00, 0x00]); // フラグ: 標準クエリ
    p.extend_from_slice(&(SERVICE_TYPES.len() as u16).to_be_bytes()); // QDCOUNT
    p.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // ANCOUNT / NSCOUNT / ARCOUNT
    for service in SERVICE_TYPES {
        for label in service.split('.') {
            p.push(label.len() as u8);
            p.extend_from_slice(label.as_bytes());
        }
        p.push(0);
        p.extend_from_slice(&TYPE_PTR.to_be_bytes());
        p.extend_from_slice(&0x8001u16.to_be_bytes()); // QU ビット + クラス IN
    }
    p
}

/// `pos` から始まる (圧縮されている可能性のある) ドメイン名を読み取る。
fn read_name(packet: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *packet.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xc0 == 0xc0 {
            let ptr = ((len & 0x3f) << 8) | *packet.get(pos + 1)? as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            jumps += 1;
            if jumps > 16 {
                return None;
            }
            pos = ptr;
            continue;
        }
        let label = packet.get(pos + 1..pos + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        pos += 1 + len;
    }
    Some((labels.join("."), end.unwrap_or(pos)))
}

/// 解析した応答: 対象のサービスに触れているかどうかと、あれば SRV のポート。
#[derive(Debug, Default, PartialEq)]
struct Answer {
    matches: bool,
    port: Option<u16>,
}

fn parse_response(packet: &[u8]) -> Option<Answer> {
    if packet.len() < 12 || packet[2] & 0x80 == 0 {
        return None; // 応答ではない
    }
    let count = |i: usize| u16::from_be_bytes([packet[i], packet[i + 1]]) as usize;
    let (qd, an, ns, ar) = (count(4), count(6), count(8), count(10));
    let mut pos = 12;
    for _ in 0..qd {
        let (_, p) = read_name(packet, pos)?;
        pos = p + 4;
    }
    let mut answer = Answer::default();
    let is_ours = |n: &str| {
        let n = n.trim_end_matches('.').to_ascii_lowercase();
        SERVICE_TYPES.iter().any(|s| n == *s || n.ends_with(&format!(".{s}")))
    };
    for _ in 0..an + ns + ar {
        let (name, p) = read_name(packet, pos)?;
        let rtype = u16::from_be_bytes([*packet.get(p)?, *packet.get(p + 1)?]);
        let rdlen = u16::from_be_bytes([*packet.get(p + 8)?, *packet.get(p + 9)?]) as usize;
        let rdata = p + 10;
        if rdata + rdlen > packet.len() {
            return None;
        }
        match rtype {
            TYPE_PTR if is_ours(&name) => answer.matches = true,
            TYPE_SRV if is_ours(&name) && rdlen >= 6 => {
                answer.matches = true;
                answer.port = Some(u16::from_be_bytes([packet[rdata + 4], packet[rdata + 5]]));
            }
            _ => {}
        }
        pos = rdata + rdlen;
    }
    Some(answer)
}

struct Probe {
    socket: Socket,
    interface: String,
}

fn open_probes() -> Vec<Probe> {
    let mut probes = Vec::new();
    let Ok(addrs) = nix::ifaddrs::getifaddrs() else { return probes };
    let query = build_query();
    let mut seen_v6 = HashSet::new();
    for ifa in addrs {
        let Some(addr) = ifa.address else { continue };
        let flags = ifa.flags;
        if flags.contains(nix::net::if_::InterfaceFlags::IFF_LOOPBACK)
            || !flags.contains(nix::net::if_::InterfaceFlags::IFF_UP)
        {
            continue;
        }
        let name = ifa.interface_name.clone();
        if let Some(v4) = addr.as_sockaddr_in() {
            let ip = v4.ip();
            let Ok(socket) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else { continue };
            if socket.bind(&SocketAddr::V4(SocketAddrV4::new(ip, 0)).into()).is_err() {
                continue;
            }
            let _ = socket.set_multicast_if_v4(&ip);
            let _ = socket.set_multicast_ttl_v4(255);
            if socket.send_to(&query, &SocketAddr::V4(SocketAddrV4::new(MDNS_V4, MDNS_PORT)).into()).is_ok() {
                probes.push(Probe { socket, interface: name });
            }
        } else if let Some(v6) = addr.as_sockaddr_in6() {
            let ip = v6.ip();
            // インターフェースごとに、そのリンクローカルアドレスから 1 回ずつ問い合わせる。
            if ip.segments()[0] & 0xffc0 != 0xfe80 {
                continue;
            }
            let Ok(index) = nix::net::if_::if_nametoindex(name.as_str()) else { continue };
            if !seen_v6.insert(index) {
                continue;
            }
            let Ok(socket) = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)) else { continue };
            let _ = socket.set_only_v6(true);
            if socket.bind(&SocketAddr::V6(SocketAddrV6::new(ip, 0, 0, index)).into()).is_err() {
                continue;
            }
            let _ = socket.set_multicast_if_v6(index);
            let _ = socket.set_multicast_hops_v6(255);
            if socket.send_to(&query, &SocketAddr::V6(SocketAddrV6::new(MDNS_V6, MDNS_PORT, 0, index)).into()).is_ok() {
                probes.push(Probe { socket, interface: name });
            }
        }
    }
    probes
}

/// `wait` の間ネットワーク上の端末を探し、それぞれの端末情報を読み取って確かめる。
pub fn discover(wait: Duration) -> Vec<DiscoveredDevice> {
    let probes = open_probes();
    let usb_ifaces: HashSet<String> = crate::usb::find_network_interfaces().into_iter().map(|i| i.name).collect();
    let mut found: Vec<DiscoveredDevice> = Vec::new();
    let deadline = Instant::now() + wait;
    let mut resent = false;

    for p in &probes {
        let _ = p.socket.set_nonblocking(true);
    }
    while Instant::now() < deadline {
        let mut any = false;
        for probe in &probes {
            let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 9000];
            while let Ok((n, from)) = probe.socket.recv_from(&mut buf) {
                any = true;
                // SAFETY: recv_from が先頭の n バイトを初期化済み。
                let data: Vec<u8> = buf[..n].iter().map(|b| unsafe { b.assume_init() }).collect();
                let Some(answer) = parse_response(&data) else { continue };
                if !answer.matches {
                    continue;
                }
                let Some(from) = from.as_socket() else { continue };
                let address = DeviceAddress::from_socket_addr(from);
                if found.iter().any(|d| d.address == address) {
                    continue;
                }
                found.push(DiscoveredDevice {
                    address,
                    port: answer.port.unwrap_or(HTTP_PORT),
                    transport: Transport::of_interface(&probe.interface, &usb_ifaces),
                    interface: Some(probe.interface.clone()),
                    info: None,
                });
            }
        }
        // 起動直後のインターフェースに備えて、途中で 1 回送り直す。
        if !resent && Instant::now() + wait / 2 >= deadline {
            resent = true;
            let query = build_query();
            for probe in &probes {
                let target: SocketAddr = match probe.socket.local_addr().ok().and_then(|a| a.as_socket()) {
                    Some(SocketAddr::V4(_)) => SocketAddr::V4(SocketAddrV4::new(MDNS_V4, MDNS_PORT)),
                    Some(SocketAddr::V6(v6)) => SocketAddr::V6(SocketAddrV6::new(MDNS_V6, MDNS_PORT, 0, v6.scope_id())),
                    None => continue,
                };
                let _ = probe.socket.send_to(&query, &target.into());
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // 確認と情報の補完を並列に行う。
    std::thread::scope(|s| {
        for dev in found.iter_mut() {
            s.spawn(move || {
                dev.info = device_information(&dev.address, Duration::from_secs(3)).ok();
            });
        }
    });
    // 確認済みの端末を優先し、同じ経路で同じシリアル番号の重複を除く。端末は IPv4 と IPv6 の両方で
    // 応答する。USB はリンクローカルだけで IPv6 が安定しているが、Wi-Fi / Bluetooth では IPv4 を優
    // 先する。
    found.sort_by_key(|d| {
        let v4 = matches!(d.address.ip, IpAddr::V4(_));
        (d.transport, d.info.is_none(), if d.transport == Transport::Usb { v4 } else { !v4 })
    });
    let mut seen = HashSet::new();
    found.retain(|d| {
        let key = (d.info.as_ref().and_then(|i| i.serial_number.clone()), d.transport);
        key.0.is_none() || seen.insert(key)
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_layout() {
        let q = build_query();
        assert_eq!(&q[4..6], &[0, 3]);
        assert_eq!(read_name(&q, 12).unwrap().0, "_dp_fujitsu._tcp.local");
    }

    #[test]
    fn parse_ptr_and_srv() {
        // PTR と SRV を含む応答。SRV の名前は PTR の rdata を参照して圧縮している。
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 2, 0, 0, 0, 0];
        // PTR: _dp_fujitsu._tcp.local -> Quaderno._dp_fujitsu._tcp.local
        let svc_off = p.len();
        for l in ["_dp_fujitsu", "_tcp", "local"] {
            p.push(l.len() as u8);
            p.extend_from_slice(l.as_bytes());
        }
        p.push(0);
        p.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 120]);
        let inst = b"Quaderno";
        p.extend_from_slice(&((inst.len() + 1 + 2) as u16).to_be_bytes());
        let inst_off = p.len();
        p.push(inst.len() as u8);
        p.extend_from_slice(inst);
        p.extend_from_slice(&[0xc0, svc_off as u8]);
        // SRV
        p.extend_from_slice(&[0xc0, inst_off as u8]);
        p.extend_from_slice(&[0, 33, 0x80, 1, 0, 0, 0, 120, 0, 8, 0, 0, 0, 0, 0x1f, 0x90, 0xc0, 12]);
        let a = parse_response(&p).unwrap();
        assert_eq!(a, Answer { matches: true, port: Some(8080) });
    }

    #[test]
    fn ignores_queries() {
        assert!(parse_response(&build_query()).is_none());
    }
}

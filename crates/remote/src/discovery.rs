//! Finding the PC on the local network when its address changed (the router gave it another one, the phone's
//! hotspot was started again, a VPN adapter took the route): the phone asks by UDP broadcast and the PC answers from
//! where it is now. The PC's key never changes, so the phone asks for that key. Also how a phone that cannot scan
//! the QR code finds a PC that is waiting to be paired with a typed code.
//!
//! One short plain-text datagram each way. Nothing in it is secret: the PC's public key is in the QR code too, and
//! every connection is still checked by the Noise handshake (only paired phones, or the right one-time code, get in).
//!   phone: "ANYTASK1 FIND <tag>"   the PC with that key: "ANYTASK1 HERE <port>"
//!   phone: "ANYTASK1 PAIR"         a PC waiting for a code: "ANYTASK1 OPEN <port> <public key> <name>"

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

const PREFIX: &str = "ANYTASK1 ";

/// The PC's public key, shortened: what the phone asks for.
pub fn tag(public: &[u8]) -> String {
    public.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A PC that answered: where to connect, and (for a PC waiting for a code) its key and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub address: String,
    pub key: Option<String>,
    pub name: Option<String>,
}

/// What the PC answers with.
#[derive(Debug, Clone)]
pub struct Answer {
    pub tag: String,
    /// The port phones connect to.
    pub port: u16,
    /// While a pairing code is valid: the PC's public key (base64) and its name.
    pub open: Option<(String, String)>,
}

/// The PC's reply to one datagram; None for anything else (another PC's tag, noise, no pairing open).
pub fn reply(request: &[u8], answer: &Answer) -> Option<String> {
    let rest = std::str::from_utf8(request).ok()?.trim().strip_prefix(PREFIX)?;
    if let Some(tag) = rest.strip_prefix("FIND ") {
        (tag.trim() == answer.tag).then(|| format!("{PREFIX}HERE {}", answer.port))
    } else if rest == "PAIR" {
        answer.open.as_ref().map(|(key, name)| format!("{PREFIX}OPEN {} {key} {}", answer.port, name.replace(['\r', '\n'], " ")))
    } else {
        None
    }
}

/// A PC's reply, from the address it came from.
pub fn parse_reply(text: &str, from: IpAddr) -> Option<Found> {
    let rest = text.trim().strip_prefix(PREFIX)?;
    if let Some(port) = rest.strip_prefix("HERE ") {
        let port: u16 = port.trim().parse().ok()?;
        return Some(Found { address: SocketAddr::new(from, port).to_string(), key: None, name: None });
    }
    let mut parts = rest.strip_prefix("OPEN ")?.splitn(3, ' ');
    let port: u16 = parts.next()?.parse().ok()?;
    let key = parts.next().filter(|k| !k.is_empty())?.to_string();
    let name = parts.next().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("PC").to_string();
    Some(Found { address: SocketAddr::new(from, port).to_string(), key: Some(key), name: Some(name) })
}

/// Answers phones for as long as the task runs. `answer` is asked for every datagram, so a pairing window that opens
/// or closes counts at once.
pub async fn respond(socket: UdpSocket, answer: impl Fn() -> Answer + Send + Sync + 'static) {
    let mut buf = [0u8; 256];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, from)) => {
                if let Some(text) = reply(&buf[..n], &answer()) {
                    let _ = socket.send_to(text.as_bytes(), from).await;
                }
            }
            // Windows reports "connection reset" on a UDP socket after one of its replies found no listener (anyone
            // can cause that); the socket still works. Other errors wait a moment; the loop never ends by itself.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

/// Sends `message` to `targets` (twice: a datagram can be lost) and collects the answers for `wait`.
pub async fn ask(message: &str, targets: &[SocketAddr], wait: Duration) -> Vec<Found> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await else { return Vec::new() };
    let _ = socket.set_broadcast(true);
    for _ in 0..2 {
        for target in targets {
            let _ = socket.send_to(message.as_bytes(), target).await;
        }
    }
    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                if let Some(pc) = std::str::from_utf8(&buf[..n]).ok().and_then(|t| parse_reply(t, from.ip()))
                    && !found.contains(&pc)
                {
                    found.push(pc);
                }
            }
            Ok(Err(_)) => continue,
            Err(_) => break,
        }
    }
    found
}

/// Everyone on the networks this device is on: 255.255.255.255 and each network's own broadcast address (a phone
/// sharing its hotspot has the PC on the hotspot's network, not on the one its default route uses).
pub fn broadcast_targets(port: u16) -> Vec<SocketAddr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    for (ip, broadcast) in interfaces() {
        let b = broadcast.unwrap_or_else(|| {
            let o = ip.octets();
            Ipv4Addr::new(o[0], o[1], o[2], 255)
        });
        if !out.contains(&b) {
            out.push(b);
        }
    }
    if let Some(ip) = crate::lan_address().and_then(|a| a.parse::<Ipv4Addr>().ok()) {
        let o = ip.octets();
        let b = Ipv4Addr::new(o[0], o[1], o[2], 255);
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out.into_iter().map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, port))).collect()
}

/// Asks for the PC with this public key; the addresses ("ip:port") it answered from.
pub async fn find(pc_key: &[u8], port: u16, wait: Duration) -> Vec<String> {
    ask(&format!("{PREFIX}FIND {}", tag(pc_key)), &broadcast_targets(port), wait).await.into_iter().map(|f| f.address).collect()
}

/// PCs waiting to be paired with a typed code.
pub async fn find_open(port: u16, wait: Duration) -> Vec<Found> {
    ask(&format!("{PREFIX}PAIR"), &broadcast_targets(port), wait).await.into_iter().filter(|f| f.key.is_some()).collect()
}

/// A Wake-on-LAN "magic packet" for a hardware address ("AA:BB:CC:DD:EE:FF" or with dashes): six 0xFF bytes, then
/// the address sixteen times.
pub fn magic_packet(mac: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = mac.split([':', '-']).map(|b| u8::from_str_radix(b, 16)).collect::<Result<_, _>>().ok()?;
    if bytes.len() != 6 {
        return None;
    }
    let mut packet = vec![0xFF; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&bytes);
    }
    Some(packet)
}

/// Sends magic packets for `macs` to everyone on this device's networks and to the networks of the PC's last
/// addresses (ports 9 and 7). Returns how many packets went out. The PC wakes only if its network adapter and BIOS
/// allow it, and only from the same network.
pub async fn wake(macs: &[String], pc_addresses: &[String]) -> usize {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await else { return 0 };
    let _ = socket.set_broadcast(true);
    let mut targets: Vec<SocketAddr> = Vec::new();
    for port in [9, 7] {
        targets.extend(broadcast_targets(port));
        for address in pc_addresses {
            if let Some(ip) = address.split(':').next().and_then(|h| h.parse::<Ipv4Addr>().ok()) {
                let o = ip.octets();
                targets.push(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(o[0], o[1], o[2], 255), port)));
            }
        }
    }
    targets.dedup();
    let mut sent = 0;
    for packet in macs.iter().filter_map(|m| magic_packet(m)) {
        for target in &targets {
            if socket.send_to(&packet, target).await.is_ok() {
                sent += 1;
            }
        }
    }
    sent
}

/// This device's IPv4 addresses (not loopback) with their broadcast address when the system gives one.
#[cfg(unix)]
fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs fills a linked list that is only read here and freed with freeifaddrs.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut cursor = list;
        while !cursor.is_null() {
            let entry = &*cursor;
            cursor = entry.ifa_next;
            if entry.ifa_addr.is_null() || i32::from((*entry.ifa_addr).sa_family) != libc::AF_INET {
                continue;
            }
            let ip = Ipv4Addr::from(u32::from_be((*(entry.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr));
            if ip.is_loopback() || ip.is_link_local() {
                continue;
            }
            let mask = (!entry.ifa_netmask.is_null()).then(|| u32::from_be((*(entry.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr));
            let broadcast = mask.filter(|m| *m != 0 && *m != u32::MAX).map(|m| Ipv4Addr::from(u32::from(ip) | !m));
            out.push((ip, broadcast));
        }
        libc::freeifaddrs(list);
    }
    out
}

#[cfg(not(unix))]
fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(open: bool) -> Answer {
        Answer { tag: "0102030405060708".into(), port: 47321, open: open.then(|| ("S0VZ".into(), "Desk PC".into())) }
    }

    #[test]
    fn the_pc_answers_only_for_its_own_key_and_only_while_pairing() {
        assert_eq!(tag(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), "0102030405060708");
        assert_eq!(reply(b"ANYTASK1 FIND 0102030405060708", &answer(false)).as_deref(), Some("ANYTASK1 HERE 47321"));
        assert_eq!(reply(b"ANYTASK1 FIND ffffffffffffffff", &answer(false)), None, "another PC's key");
        assert_eq!(reply(b"ANYTASK1 PAIR", &answer(false)), None, "no pairing open");
        assert_eq!(reply(b"ANYTASK1 PAIR", &answer(true)).as_deref(), Some("ANYTASK1 OPEN 47321 S0VZ Desk PC"));
        assert_eq!(reply(b"hello", &answer(true)), None);
        assert_eq!(reply(&[0xff, 0xfe], &answer(true)), None);
    }

    #[test]
    fn replies_are_read_with_the_address_they_came_from() {
        let from: IpAddr = "10.221.116.223".parse().unwrap();
        assert_eq!(parse_reply("ANYTASK1 HERE 47321", from), Some(Found { address: "10.221.116.223:47321".into(), key: None, name: None }));
        let open = parse_reply("ANYTASK1 OPEN 47321 S0VZ My Desk PC", from).unwrap();
        assert_eq!((open.key.as_deref(), open.name.as_deref()), (Some("S0VZ"), Some("My Desk PC")));
        assert_eq!(parse_reply("ANYTASK1 HERE x", from), None);
        assert_eq!(parse_reply("ANYTASK1 OPEN 1", from), None);
    }

    #[test]
    fn a_magic_packet_is_six_ff_and_the_address_sixteen_times() {
        let packet = magic_packet("A4:B1:C2:D3:E4:F5").unwrap();
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        assert_eq!(&packet[6..12], &[0xA4, 0xB1, 0xC2, 0xD3, 0xE4, 0xF5]);
        assert_eq!(&packet[96..], &[0xA4, 0xB1, 0xC2, 0xD3, 0xE4, 0xF5]);
        assert_eq!(magic_packet("a4-b1-c2-d3-e4-f5"), Some(packet));
        assert!(magic_packet("A4:B1:C2").is_none());
        assert!(magic_packet("zz:b1:c2:d3:e4:f5").is_none());
    }

    #[test]
    fn broadcasts_go_to_every_network_and_to_everyone() {
        let targets = broadcast_targets(47321);
        assert_eq!(targets[0], "255.255.255.255:47321".parse().unwrap());
        assert!(targets.iter().all(|t| t.port() == 47321));
    }
}

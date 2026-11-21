//! The PC's network adapters: the addresses phones can reach it at (all of them, not only the one the default route
//! uses: with a VPN on, that one is the VPN's), and the hardware addresses of its wired and Wi-Fi adapters, which the
//! phone needs to wake the PC with Wake-on-LAN.

use std::net::Ipv4Addr;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Adapter {
    pub name: String,
    /// The driver's description ("Intel(R) Wi-Fi 6 AX201", "Hyper-V Virtual Ethernet Adapter").
    pub description: String,
    pub ipv4: Vec<Ipv4Addr>,
    /// "AA:BB:CC:DD:EE:FF" when the adapter has a 6-byte hardware address.
    pub mac: Option<String>,
    /// A wired or Wi-Fi adapter by type (virtual machines' and some VPNs' adapters say so too; see `is_virtual`).
    pub physical: bool,
    /// It has a default gateway: the adapter the PC really uses to reach its network.
    pub gateway: bool,
}

impl Adapter {
    /// A virtual machine's or a VPN's adapter (WSL, Hyper-V, VirtualBox, VMware, TAP/TUN): its addresses are
    /// rarely reachable from the phone.
    pub fn is_virtual(&self) -> bool {
        let text = format!("{} {}", self.name, self.description).to_lowercase();
        ["virtual", "vethernet", "hyper-v", "virtualbox", "vmware", "tap-", "tap ", "wintun", "tun ", "vpn", "loopback", "pseudo", "hotspot", "speedify"]
            .iter()
            .any(|k| text.contains(k))
    }

    /// A private overlay network (Tailscale, ZeroTier): reachable from the phone anywhere when it is in it too.
    pub fn is_overlay(&self) -> bool {
        let text = format!("{} {}", self.name, self.description).to_lowercase();
        text.contains("tailscale") || text.contains("zerotier") || self.ipv4.iter().any(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
    }
}

/// Addresses a phone can reach: on the local network (10/8, 172.16/12, 192.168/16) or on a private overlay such as
/// Tailscale (100.64/10), never loopback, the self-assigned 169.254 ones, or virtual machines' and VPNs' adapters.
/// `preferred` (the address the default route uses) comes first when it belongs to a usable adapter, then adapters
/// with a gateway, then the rest.
pub fn reachable(adapters: &[Adapter], preferred: Option<&str>) -> Vec<String> {
    let usable_ip = |ip: &Ipv4Addr| {
        let o = ip.octets();
        ip.is_private() || (o[0] == 100 && (64..128).contains(&o[1]))
    };
    // A virtual adapter with a gateway is the real way out (Hyper-V's external switch carries the PC's LAN address).
    let kept: Vec<&Adapter> = adapters.iter().filter(|a| a.is_overlay() || a.gateway || !a.is_virtual()).collect();
    let mut out: Vec<String> = Vec::new();
    let preferred = preferred.and_then(|p| p.parse::<Ipv4Addr>().ok()).filter(usable_ip);
    // The route's address is kept unless it is on a virtual adapter without a gateway (WSL's); the phone tries
    // every address at once, so an extra one costs nothing.
    if let Some(p) = preferred.filter(|p| !adapters.iter().any(|a| a.ipv4.contains(p) && a.is_virtual() && !a.gateway && !a.is_overlay())) {
        out.push(p.to_string());
    }
    let mut sorted = kept;
    sorted.sort_by_key(|a| (!a.gateway, a.is_virtual() && !a.is_overlay(), !a.physical, a.is_overlay()));
    for ip in sorted.iter().flat_map(|a| a.ipv4.iter()).filter(|ip| usable_ip(ip)) {
        let text = ip.to_string();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// Hardware addresses of the wired and Wi-Fi adapters that are up.
pub fn wake_addresses(adapters: &[Adapter]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for mac in adapters.iter().filter(|a| a.physical && !a.is_virtual()).filter_map(|a| a.mac.clone()) {
        if mac != "00:00:00:00:00:00" && !out.contains(&mac) {
            out.push(mac);
        }
    }
    out
}

/// The adapters that are up, with their IPv4 addresses.
#[cfg(windows)]
pub fn adapters() -> Vec<Adapter> {
    use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let mut size: u32 = 16 * 1024;
    let mut buffer: Vec<u64>;
    // SAFETY: the buffer is sized by the API (u64 elements keep it aligned for the structures); the linked list it
    // fills is only read while the buffer lives.
    unsafe {
        loop {
            buffer = vec![0u64; (size as usize).div_ceil(8)];
            let result = GetAdaptersAddresses(
                u32::from(AF_INET.0),
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_INCLUDE_GATEWAYS,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size,
            );
            if result == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if result != 0 {
                return Vec::new();
            }
            break;
        }
        let mut out = Vec::new();
        let mut cursor = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !cursor.is_null() {
            let adapter = &*cursor;
            cursor = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp {
                continue;
            }
            let mut ipv4 = Vec::new();
            let mut unicast = adapter.FirstUnicastAddress;
            while !unicast.is_null() {
                let address = (*unicast).Address.lpSockaddr;
                if !address.is_null() && (*address).sa_family == AF_INET {
                    let sin = &*address.cast::<SOCKADDR_IN>();
                    ipv4.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr)));
                }
                unicast = (*unicast).Next;
            }
            let mac = (adapter.PhysicalAddressLength == 6).then(|| adapter.PhysicalAddress[..6].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"));
            // IF_TYPE_ETHERNET_CSMACD (6) and IF_TYPE_IEEE80211 (71).
            let physical = matches!(adapter.IfType, 6 | 71);
            out.push(Adapter {
                name: adapter.FriendlyName.to_string().unwrap_or_default(),
                description: adapter.Description.to_string().unwrap_or_default(),
                ipv4,
                mac,
                physical,
                gateway: !adapter.FirstGatewayAddress.is_null(),
            });
        }
        out
    }
}

#[cfg(not(windows))]
pub fn adapters() -> Vec<Adapter> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, description: &str, ips: &[&str], mac: Option<&str>, physical: bool, gateway: bool) -> Adapter {
        Adapter { name: name.into(), description: description.into(), ipv4: ips.iter().map(|i| i.parse().unwrap()).collect(), mac: mac.map(Into::into), physical, gateway }
    }

    #[test]
    fn phones_get_every_reachable_address_and_the_wired_adapters_hardware_address() {
        // As on this PC (11-12): Wi-Fi on a hotspot, several VPN adapters with self-assigned addresses; plus WSL's
        // Hyper-V adapter, which reports itself as Ethernet with a private address no phone can reach.
        let adapters = vec![
            adapter("HotspotShield Network Adapter", "HotspotShield Network Adapter", &["169.254.4.25"], Some("00:FF:11:22:33:44"), true, false),
            adapter("vEthernet (WSL)", "Hyper-V Virtual Ethernet Adapter", &["172.29.64.1"], Some("00:15:5D:01:02:03"), true, false),
            adapter("Tailscale", "Tailscale Tunnel", &["100.101.102.103"], None, false, false),
            adapter("Ethernet", "Realtek PCIe GbE Family Controller", &["169.254.255.40"], Some("10:20:30:40:50:60"), true, false),
            adapter("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", &["10.221.116.223"], Some("A4:B1:C2:D3:E4:F5"), true, true),
            adapter("Loopback", "Software Loopback Interface 1", &["127.0.0.1"], None, false, false),
        ];
        assert_eq!(reachable(&adapters, Some("10.221.116.223")), ["10.221.116.223", "100.101.102.103"]);
        assert_eq!(reachable(&adapters, Some("172.29.64.1")), ["10.221.116.223", "100.101.102.103"], "a virtual adapter's address is never offered");
        assert_eq!(reachable(&adapters, Some("8.8.8.8")), ["10.221.116.223", "100.101.102.103"], "a public address is not offered");
        assert_eq!(wake_addresses(&adapters), ["10:20:30:40:50:60", "A4:B1:C2:D3:E4:F5"]);
        assert_eq!(reachable(&adapters, Some("192.168.8.20")), ["192.168.8.20", "10.221.116.223", "100.101.102.103"], "an address of no listed adapter is kept");
        // Hyper-V's external switch: the LAN address sits on a virtual adapter that has the gateway.
        let hyperv = vec![
            adapter("vEthernet (External)", "Hyper-V Virtual Ethernet Adapter", &["192.168.1.40"], Some("00:15:5D:AA:BB:CC"), true, true),
            adapter("vEthernet (WSL)", "Hyper-V Virtual Ethernet Adapter #2", &["172.29.64.1"], Some("00:15:5D:01:02:03"), true, false),
        ];
        assert_eq!(reachable(&hyperv, Some("192.168.1.40")), ["192.168.1.40"]);
        assert_eq!(reachable(&hyperv, None), ["192.168.1.40"]);
    }

    #[cfg(windows)]
    #[test]
    fn this_pc_has_adapters() {
        let all = adapters();
        assert!(!all.is_empty(), "every Windows PC has an adapter that is up");
        assert!(all.iter().all(|a| !a.name.is_empty()), "{all:?}");
    }
}

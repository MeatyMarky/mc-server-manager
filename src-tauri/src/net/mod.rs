//! Addresses, ports and port forwarding: how somebody else reaches this server.
//!
//! "How do my friends join?" is the question this app was least able to answer.
//! The parts of the answer live in different places — the LAN address is on an
//! adapter, the VPN address is on another adapter that looks the same, the
//! public address is only known to the router, and whether the port is open is
//! knowable only from outside. This module gathers them and, where it cannot
//! know something, says so instead of guessing.

pub mod check;
pub mod classify;
pub mod upnp;

use std::net::Ipv4Addr;

use serde::Serialize;
use ts_rs::TS;

pub use check::{external_reachability, local_port_state, LocalPort, Reachability};
pub use classify::{classify, joinable, AddressKind};

/// One address somebody could type into Minecraft's "Add server" box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct NetAddress {
    /// The bare address, without a port.
    pub address: String,
    /// `address` or `address:port` — what is actually copied.
    pub joinable: String,
    pub kind: AddressKind,
    /// The VPN product this block belongs to, when the block names one.
    pub network: Option<String>,
    /// Who can use this address, in one sentence.
    pub audience: String,
    /// The adapter it came from, as the OS names it.
    pub interface: String,
}

/// The routers this machine sends traffic through, as manual port-forwarding
/// instructions need one to name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Gateway {
    pub address: String,
    pub interface: String,
}

/// Every local IPv4 address, described, plus the gateway to name in the manual
/// instructions.
///
/// Loopback is left out: `127.0.0.1` is not an answer to "how does my friend
/// join", and listing it invites somebody to send it to one.
pub fn addresses(port: u16) -> (Vec<NetAddress>, Option<Gateway>) {
    let interfaces = netdev::get_interfaces();
    from_interfaces(&interfaces, port)
}

/// The pure half, so the shape of the list is testable without a machine that
/// happens to have the right adapters.
fn from_interfaces(interfaces: &[netdev::Interface], port: u16) -> (Vec<NetAddress>, Option<Gateway>) {
    let mut found = Vec::new();
    let mut gateway = None;

    for interface in interfaces {
        if interface.is_loopback() || !interface.is_up() {
            continue;
        }

        let name = interface
            .friendly_name
            .clone()
            .unwrap_or_else(|| interface.name.clone());

        for net in &interface.ipv4 {
            let ip: Ipv4Addr = net.addr();
            if ip.is_loopback() || ip.is_unspecified() {
                continue;
            }

            let described = classify(ip);
            found.push(NetAddress {
                address: ip.to_string(),
                joinable: joinable(&ip.to_string(), port),
                kind: described.kind,
                network: described.network.map(str::to_string),
                audience: described.audience.to_string(),
                interface: name.clone(),
            });
        }

        if gateway.is_none() {
            if let Some(device) = interface.gateway.as_ref() {
                if let Some(ip) = device.ipv4.first() {
                    gateway = Some(Gateway {
                        address: ip.to_string(),
                        interface: name.clone(),
                    });
                }
            }
        }
    }

    // LAN first, then VPNs, then anything else: the LAN address is the answer
    // for most people asking, and a list that opens with a Hamachi address
    // reads as though that is the normal way in.
    found.sort_by_key(|entry| match entry.kind {
        AddressKind::Lan => 0,
        AddressKind::Vpn => 1,
        AddressKind::Public => 2,
        AddressKind::LinkLocal => 3,
        AddressKind::Loopback => 4,
    });

    (found, gateway)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdev::ipnet::Ipv4Net;
    use netdev::{Interface, MacAddr, NetworkDevice};

    /// A machine with a LAN adapter, a Radmin adapter, a Tailscale adapter and
    /// a disconnected one — the shape this app is actually looked at on.
    fn machine() -> Vec<Interface> {
        let mut ethernet = Interface::dummy();
        ethernet.name = "eth0".into();
        ethernet.friendly_name = Some("Ethernet".into());
        ethernet.ipv4 = vec![Ipv4Net::new("192.168.1.24".parse().unwrap(), 24).unwrap()];
        ethernet.flags = up();
        ethernet.gateway = Some(NetworkDevice {
            mac_addr: MacAddr::zero(),
            ipv4: vec!["192.168.1.1".parse().unwrap()],
            ipv6: vec![],
        });

        let mut radmin = Interface::dummy();
        radmin.name = "radmin".into();
        radmin.friendly_name = Some("Radmin VPN".into());
        radmin.ipv4 = vec![Ipv4Net::new("26.31.4.9".parse().unwrap(), 8).unwrap()];
        radmin.flags = up();

        let mut tailscale = Interface::dummy();
        tailscale.name = "tailscale0".into();
        tailscale.ipv4 = vec![Ipv4Net::new("100.101.102.103".parse().unwrap(), 32).unwrap()];
        tailscale.flags = up();

        let mut unplugged = Interface::dummy();
        unplugged.name = "eth1".into();
        unplugged.ipv4 = vec![Ipv4Net::new("192.168.9.9".parse().unwrap(), 24).unwrap()];
        unplugged.flags = 0;

        vec![ethernet, radmin, tailscale, unplugged]
    }

    /// The platform's own "this adapter is up" bit, rather than a number
    /// copied out of a header that differs between the two targets.
    ///
    /// The cast is redundant on Windows, where the constant is already a
    /// `u32`, and load-bearing on Unix, where it is a `c_int`.
    #[allow(clippy::unnecessary_cast)]
    fn up() -> u32 {
        netdev::interface::flags::IFF_UP as u32
    }

    #[test]
    fn lan_comes_first_and_each_vpn_is_named() {
        let (found, gateway) = from_interfaces(&machine(), 25565);

        let labels: Vec<_> = found
            .iter()
            .map(|entry| (entry.address.as_str(), entry.kind, entry.network.as_deref()))
            .collect();

        assert_eq!(
            labels,
            vec![
                ("192.168.1.24", AddressKind::Lan, None),
                ("26.31.4.9", AddressKind::Vpn, Some("Radmin VPN")),
                ("100.101.102.103", AddressKind::Vpn, Some("Tailscale")),
            ]
        );
        assert_eq!(gateway.unwrap().address, "192.168.1.1");
    }

    #[test]
    fn a_down_adapter_is_not_offered() {
        let (found, _) = from_interfaces(&machine(), 25565);
        assert!(!found.iter().any(|entry| entry.address == "192.168.9.9"));
    }

    #[test]
    fn a_non_default_port_travels_with_the_address() {
        let (found, _) = from_interfaces(&machine(), 25570);
        assert_eq!(found[0].joinable, "192.168.1.24:25570");
    }
}

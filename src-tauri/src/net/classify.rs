//! What an address means to the person reading it.
//!
//! A list of raw IPv4 addresses is useless to somebody who wants to tell a
//! friend how to join: `192.168.1.24` works for the people in the house,
//! `26.31.4.9` works for whoever is in the same Radmin network, and neither
//! works from the open internet. The label has to say which.
//!
//! VPN ranges are recognised by their address block rather than by the
//! adapter's name, because adapter names are localised, renamed by users, and
//! differ between driver versions — the block is the stable part.

use std::net::Ipv4Addr;

use serde::Serialize;
use ts_rs::TS;

/// Who can reach a server at this address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum AddressKind {
    /// This machine only.
    Loopback,
    /// Other devices on the same home or office network.
    Lan,
    /// Whoever is joined to the same VPN network.
    Vpn,
    /// Reachable from the internet, at least as far as this machine can tell.
    Public,
    /// An address the machine gave itself because no DHCP server answered.
    LinkLocal,
}

/// One address, described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub kind: AddressKind,
    /// The network's name where the block identifies one, e.g. "Radmin VPN".
    pub network: Option<&'static str>,
    /// One sentence: who can use this address.
    pub audience: &'static str,
}

/// Ranges that belong to a named VPN product.
///
/// Radmin and Hamachi each took a whole /8 that is publicly allocated but never
/// routed to them, so an address in one of those blocks on a local adapter is
/// their virtual network and nothing else. Tailscale hands out addresses from
/// the carrier-grade NAT block, which is shared with a few ISPs — hence
/// "usually", not a promise.
fn vpn_network(ip: Ipv4Addr) -> Option<(&'static str, &'static str)> {
    let [a, b, ..] = ip.octets();
    match (a, b) {
        (25, _) => Some((
            "Hamachi",
            "Anyone joined to the same Hamachi network can use this.",
        )),
        (26, _) => Some((
            "Radmin VPN",
            "Anyone joined to the same Radmin network can use this.",
        )),
        (100, 64..=127) => Some((
            "Tailscale",
            "Anyone on the same Tailscale network (tailnet) can use this.",
        )),
        _ => None,
    }
}

/// What an address is, and who it is good for.
pub fn classify(ip: Ipv4Addr) -> Classified {
    if ip.is_loopback() {
        return Classified {
            kind: AddressKind::Loopback,
            network: None,
            audience: "Only this computer can use this address.",
        };
    }

    if ip.is_link_local() {
        return Classified {
            kind: AddressKind::LinkLocal,
            network: None,
            audience: "No router answered, so this address rarely works for anyone.",
        };
    }

    if let Some((network, audience)) = vpn_network(ip) {
        return Classified {
            kind: AddressKind::Vpn,
            network: Some(network),
            audience,
        };
    }

    if ip.is_private() {
        return Classified {
            kind: AddressKind::Lan,
            network: None,
            audience: "Anyone on the same network as this computer can use this.",
        };
    }

    Classified {
        kind: AddressKind::Public,
        network: None,
        audience: "Reachable from the internet if the port is open.",
    }
}

/// How an address is written when it is handed to somebody: `host:port`, with
/// the port left off when it is the one Minecraft assumes.
pub fn joinable(address: &str, port: u16) -> String {
    if port == crate::process::port::DEFAULT_PORT {
        address.to_string()
    } else {
        format!("{address}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One address per adapter range this app claims to recognise. These are
    /// the exact blocks the products hand out, and a change to the table has
    /// to break a test rather than mislabel somebody's network.
    #[test]
    fn every_recognised_range_gets_its_name() {
        let cases = [
            ("192.168.1.24", AddressKind::Lan, None),
            ("10.0.0.7", AddressKind::Lan, None),
            ("172.16.4.1", AddressKind::Lan, None),
            ("172.31.255.254", AddressKind::Lan, None),
            ("25.14.200.3", AddressKind::Vpn, Some("Hamachi")),
            ("26.31.4.9", AddressKind::Vpn, Some("Radmin VPN")),
            ("100.64.0.1", AddressKind::Vpn, Some("Tailscale")),
            ("100.101.102.103", AddressKind::Vpn, Some("Tailscale")),
            ("100.127.255.254", AddressKind::Vpn, Some("Tailscale")),
            ("127.0.0.1", AddressKind::Loopback, None),
            ("169.254.1.5", AddressKind::LinkLocal, None),
            ("81.2.69.142", AddressKind::Public, None),
        ];

        for (address, kind, network) in cases {
            let found = classify(address.parse().unwrap());
            assert_eq!(found.kind, kind, "{address}");
            assert_eq!(found.network, network, "{address}");
            assert!(found.audience.ends_with('.'), "{address}");
        }
    }

    /// 172.32 is not private, and 100.128 is past the carrier-grade block -
    /// both are one step outside a range, which is where a sloppy check fails.
    #[test]
    fn addresses_just_outside_a_range_are_not_claimed() {
        assert_eq!(classify("172.32.0.1".parse().unwrap()).kind, AddressKind::Public);
        assert_eq!(classify("100.63.255.255".parse().unwrap()).kind, AddressKind::Public);
        assert_eq!(classify("100.128.0.1".parse().unwrap()).kind, AddressKind::Public);
        assert_eq!(classify("24.0.0.1".parse().unwrap()).kind, AddressKind::Public);
        assert_eq!(classify("27.0.0.1".parse().unwrap()).kind, AddressKind::Public);
    }

    #[test]
    fn the_default_port_is_left_off_a_joinable_address() {
        assert_eq!(joinable("192.168.1.24", 25565), "192.168.1.24");
        assert_eq!(joinable("192.168.1.24", 25566), "192.168.1.24:25566");
    }
}

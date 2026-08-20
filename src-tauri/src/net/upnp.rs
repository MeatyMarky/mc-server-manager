//! Asking the router to forward the port, when it will listen.
//!
//! UPnP is not a feature that can be relied on: plenty of routers ship with it
//! off, ISP-supplied boxes often remove it, and a machine behind two routers or
//! carrier-grade NAT cannot be helped by it at all. So every failure here ends
//! in a sentence a person can act on, and the manual instructions stay on
//! screen next to the button rather than appearing only after it fails.
//!
//! Nothing here runs on its own. A mapping is a change to somebody's router,
//! made only when they press the button.

use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::{PortMappingProtocol, SearchOptions};
use serde::Serialize;
use ts_rs::TS;

use crate::error::AppResult;

/// How long a mapping lasts before the router drops it, in seconds.
///
/// Deliberately not infinite: a permanent mapping outlives the server, the
/// app, and often the person's memory of having made it. Renewal happens the
/// next time they press the button.
pub const LEASE_SECONDS: u32 = 12 * 60 * 60;

/// Router discovery is a multicast question with no guaranteed answer, so it
/// gets a short deadline rather than the crate's ten seconds.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(3);

/// What the router said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct MappingResult {
    pub ok: bool,
    /// One sentence naming what happened, router included where it named itself.
    pub detail: String,
    /// The public address the router reports, when it reports one.
    pub external_ip: Option<String>,
    /// How long the mapping lasts, in hours, when one was made.
    #[ts(type = "number | null")]
    pub lease_hours: Option<i64>,
}

/// Whether a router that speaks UPnP can be found at all.
///
/// Separate from mapping so the tab can say "no router answered" before
/// somebody presses a button that would then fail.
pub async fn discover() -> AppResult<Option<String>> {
    match search().await {
        Ok(gateway) => Ok(Some(gateway.addr.to_string())),
        Err(_) => Ok(None),
    }
}

async fn search() -> Result<Gateway<Tokio>, igd_next::SearchError> {
    search_gateway(SearchOptions {
        timeout: Some(SEARCH_TIMEOUT),
        ..SearchOptions::default()
    })
    .await
}

/// Asks the router to forward `port` to this machine.
///
/// `local_ip` is the LAN address the server is reachable at — the router maps
/// to an address, not to "whoever asked", and on a machine with a VPN adapter
/// the wrong one produces a mapping that silently goes nowhere.
pub async fn map_port(local_ip: std::net::Ipv4Addr, port: u16) -> AppResult<MappingResult> {
    let Ok(gateway) = search().await else {
        return Ok(MappingResult {
            ok: false,
            detail: "No router answered the UPnP request. Many routers have it switched off, \
                     and some ISP-supplied ones do not have it at all — the manual steps below \
                     do the same job."
                .to_string(),
            external_ip: None,
            lease_hours: None,
        });
    };

    let external_ip = gateway.get_external_ip().await.ok().map(|ip| ip.to_string());
    let local = SocketAddr::V4(SocketAddrV4::new(local_ip, port));

    match gateway
        .add_port(
            PortMappingProtocol::TCP,
            port,
            local,
            LEASE_SECONDS,
            "Minecraft server (MC Server Manager)",
        )
        .await
    {
        Ok(()) => Ok(MappingResult {
            ok: true,
            detail: format!(
                "The router at {} is forwarding port {port} to {local_ip}.",
                gateway.addr
            ),
            external_ip,
            lease_hours: Some(i64::from(LEASE_SECONDS) / 3600),
        }),
        Err(error) => Ok(MappingResult {
            ok: false,
            detail: format!(
                "The router at {} refused the request: {error}. The manual steps below do the \
                 same job.",
                gateway.addr
            ),
            external_ip,
            lease_hours: None,
        }),
    }
}

/// Removes a mapping this app made.
pub async fn unmap_port(port: u16) -> AppResult<MappingResult> {
    let Ok(gateway) = search().await else {
        return Ok(MappingResult {
            ok: false,
            detail: "No router answered, so there is nothing to remove from here.".to_string(),
            external_ip: None,
            lease_hours: None,
        });
    };

    match gateway.remove_port(PortMappingProtocol::TCP, port).await {
        Ok(()) => Ok(MappingResult {
            ok: true,
            detail: format!("The router is no longer forwarding port {port}."),
            external_ip: None,
            lease_hours: None,
        }),
        Err(error) => Ok(MappingResult {
            ok: false,
            detail: format!("The router refused to remove the mapping: {error}."),
            external_ip: None,
            lease_hours: None,
        }),
    }
}

/// The public address as the router sees it.
///
/// Asked of the router rather than of a website: it is the same answer without
/// this machine's address travelling to a third party.
pub async fn external_ip() -> AppResult<Option<String>> {
    let Ok(gateway) = search().await else {
        return Ok(None);
    };
    Ok(gateway.get_external_ip().await.ok().map(|ip| match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6.to_string(),
    }))
}

/// The manual steps, written for the router this machine actually uses.
///
/// Generic port-forwarding advice is useless at the one moment it is needed —
/// "log into your router" is not an address, and the person reading this has
/// never seen theirs. The gateway address is a link they can click.
pub fn manual_steps(gateway: Option<&str>, local_ip: &str, port: u16) -> Vec<String> {
    let router = gateway.unwrap_or("your router's address");
    vec![
        format!("Open http://{router} in a browser and sign in to the router."),
        "Find the section called Port Forwarding, Virtual Server, or NAT.".to_string(),
        format!("Forward external port {port} (TCP) to {local_ip} port {port}."),
        "Save, then use the outside check above to confirm it worked.".to_string(),
        format!(
            "If the check still fails, the port may be blocked by this computer's firewall, or \
             your connection may not have a public address of its own — ask your provider about \
             CGNAT before changing more router settings."
        ),
    ]
}

/// Whether an address can be reached from the internet at all.
///
/// A private public-IP is the signature of carrier-grade NAT: the router's
/// "external" address is itself behind another one, and no amount of port
/// forwarding on this router will help.
pub fn is_carrier_nat(external_ip: &str) -> bool {
    external_ip
        .parse::<std::net::Ipv4Addr>()
        .map(|ip| {
            let [a, b, ..] = ip.octets();
            ip.is_private() || matches!((a, b), (100, 64..=127))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_steps_name_the_router_when_it_is_known() {
        let steps = manual_steps(Some("192.168.1.1"), "192.168.1.24", 25565);
        assert!(steps[0].contains("http://192.168.1.1"));
        assert!(steps[2].contains("25565 (TCP) to 192.168.1.24"));
    }

    #[test]
    fn manual_steps_stay_readable_without_a_gateway() {
        let steps = manual_steps(None, "192.168.1.24", 25565);
        assert!(steps[0].contains("your router's address"));
        assert_eq!(steps.len(), 5);
    }

    /// The case where port forwarding cannot work no matter what the user does,
    /// and saying so early saves an evening.
    #[test]
    fn a_private_external_address_is_carrier_nat() {
        assert!(is_carrier_nat("100.72.0.14"));
        assert!(is_carrier_nat("10.20.30.40"));
        assert!(is_carrier_nat("192.168.0.1"));
        assert!(!is_carrier_nat("81.2.69.142"));
        assert!(!is_carrier_nat("not an address"));
    }
}

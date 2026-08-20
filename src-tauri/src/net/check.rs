//! Is the port open? Asked twice, because there are two different questions.
//!
//! From this machine we can see whether anything is listening. Whether the
//! *internet* can reach it is knowable only from outside the network, so it is
//! asked of an outside service — and when that service cannot be reached, the
//! answer is "we could not tell", never "closed". Telling somebody their port
//! forwarding is broken when the truth is that a status API was down sends
//! them off to rewrite router settings that were fine.

use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppResult;
use crate::http::Fetch;

/// How long to wait for a connection to the local server before calling it
/// unreachable. Local, so anything slower than this is not a slow network.
const LOCAL_TIMEOUT: Duration = Duration::from_millis(600);

/// The outside opinion, from a service that pings Minecraft servers.
///
/// It answers the exact question being asked — "can something on the internet
/// reach this server" — rather than the "is this TCP port open" that a generic
/// port scanner answers, and it needs nothing installed at the router.
const STATUS_API: &str = "https://api.mcsrvstat.us/3";

/// What this machine can see about its own port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum LocalPort {
    /// Something is listening, and it answered.
    Listening,
    /// Nothing is listening. For a stopped server this is normal.
    Closed,
}

/// Whether the outside world can get in — including not knowing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Reachability {
    /// `None` means the check could not be completed, which is not the same as
    /// a closed port and is never rendered as one.
    pub reachable: Option<bool>,
    /// What was found, or why nothing could be.
    pub detail: String,
    /// The address the outside service was asked about.
    pub asked_about: String,
}

/// Whether anything answers on the port, locally.
///
/// A connect rather than a bind: a bind test says "the port is free", which is
/// the opposite of what this asks, and on Windows a successful bind to the
/// wildcard address says nothing about a server bound to one interface.
pub fn local_port_state(port: u16) -> LocalPort {
    let target = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    match TcpStream::connect_timeout(&target.into(), LOCAL_TIMEOUT) {
        Ok(_) => LocalPort::Listening,
        Err(_) => LocalPort::Closed,
    }
}

#[derive(Debug, Deserialize)]
struct StatusResponse {
    online: bool,
}

/// Asks an outside service whether it can reach the server.
///
/// `host` is the public address; the service resolves and connects from its
/// own network, which is the only way to answer this honestly from inside.
pub async fn external_reachability<F: Fetch>(
    fetch: &F,
    host: &str,
    port: u16,
) -> AppResult<Reachability> {
    let asked_about = super::joinable(host, port);
    let url = format!("{STATUS_API}/{asked_about}");

    match fetch.get_json::<StatusResponse>(&url).await {
        Ok(status) if status.online => Ok(Reachability {
            reachable: Some(true),
            detail: "An outside service connected to this server, so the port is open."
                .to_string(),
            asked_about,
        }),
        Ok(_) => Ok(Reachability {
            reachable: Some(false),
            detail: "An outside service could not reach this server. If it is running, the \
                     port is not forwarded to this computer."
                .to_string(),
            asked_about,
        }),
        // A failed check is a failed check. The port may well be open.
        Err(error) => Ok(Reachability {
            reachable: None,
            detail: format!(
                "The outside check could not be completed, so this says nothing about \
                 whether the port is open ({error})."
            ),
            asked_about,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;
    use std::net::TcpListener;

    #[test]
    fn a_listening_socket_is_seen_and_a_free_port_is_not() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(local_port_state(port), LocalPort::Listening);

        drop(listener);
        assert_eq!(local_port_state(port), LocalPort::Closed);
    }

    #[tokio::test]
    async fn an_online_server_reads_as_reachable() {
        let fetch = FixtureFetch::new().route(
            "https://api.mcsrvstat.us/3/81.2.69.142",
            "mcsrvstat_online.json",
        );
        let found = external_reachability(&fetch, "81.2.69.142", 25565)
            .await
            .unwrap();
        assert_eq!(found.reachable, Some(true));
        assert_eq!(found.asked_about, "81.2.69.142");
    }

    #[tokio::test]
    async fn an_offline_answer_is_a_closed_port() {
        let fetch = FixtureFetch::new().route(
            "https://api.mcsrvstat.us/3/81.2.69.142:25570",
            "mcsrvstat_offline.json",
        );
        let found = external_reachability(&fetch, "81.2.69.142", 25570)
            .await
            .unwrap();
        assert_eq!(found.reachable, Some(false));
    }

    /// The case this module exists for: the service is unreachable, and the
    /// answer must stay "do not know" rather than collapsing into "closed".
    #[tokio::test]
    async fn a_failed_check_is_not_a_closed_port() {
        // FixtureFetch fails any URL a test did not record.
        let fetch = FixtureFetch::new();
        let found = external_reachability(&fetch, "81.2.69.142", 25565)
            .await
            .unwrap();
        assert_eq!(found.reachable, None);
        assert!(found.detail.contains("says nothing"));
    }
}

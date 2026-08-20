//! The Networking tab's commands: addresses, port checks, and the router.
//!
//! Each of these is a question the user asked out loud ("what address do I give
//! my friend", "is the port open", "can you open it"), and each answer carries
//! the reason with it, because "no" without a reason sends somebody to a forum.

use tauri::State;

use crate::error::AppResult;
use crate::instance;
use crate::net::upnp::MappingResult;
use crate::net::{self, Gateway, LocalPort, NetAddress, Reachability};
use crate::process::port;
use crate::state::AppState;

use serde::Serialize;
use ts_rs::TS;

/// Everything the tab shows before anything is checked.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct NetworkView {
    pub port: u16,
    pub addresses: Vec<NetAddress>,
    pub gateway: Option<Gateway>,
    /// True when the server's own `server.properties` has the whitelist on.
    pub whitelist_enabled: bool,
    /// True when the server is online-mode, which is worth saying next to a
    /// VPN address: cracked clients cannot join either way.
    pub online_mode: bool,
    /// What this machine can see about its own port right now.
    pub local: LocalPort,
    /// Manual port-forwarding steps, written for this machine's router.
    pub manual_steps: Vec<String>,
}

#[tauri::command]
pub async fn network_view(state: State<'_, AppState>, id: i64) -> AppResult<NetworkView> {
    let row = instance::get(&state.db, id).await?;
    let path = row.path_buf();

    // Reading the properties file is blocking work, and so is walking the
    // machine's adapter list.
    let view = tokio::task::spawn_blocking(move || {
        let configured = port::configured_port(&path);
        let properties = std::fs::read_to_string(crate::paths::server_properties_path(&path))
            .unwrap_or_default();

        let (addresses, gateway) = net::addresses(configured);
        let local_ip = addresses
            .first()
            .map(|entry| entry.address.clone())
            .unwrap_or_else(|| "this computer".to_string());

        NetworkView {
            port: configured,
            manual_steps: net::upnp::manual_steps(
                gateway.as_ref().map(|entry| entry.address.as_str()),
                &local_ip,
                configured,
            ),
            addresses,
            gateway,
            whitelist_enabled: port::read_property(&properties, "white-list")
                .map(|value| value == "true")
                .unwrap_or(false),
            online_mode: port::read_property(&properties, "online-mode")
                .map(|value| value == "true")
                .unwrap_or(true),
            local: net::local_port_state(configured),
            }
    })
    .await
    .map_err(|error| crate::error::AppError::Other(error.to_string()))?;

    Ok(view)
}

/// The public address, and what to hand somebody.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PublicAddress {
    pub address: String,
    /// `address` or `address:port` — assembled here, like every other address
    /// on this screen, rather than by the page.
    pub joinable: String,
    /// True when the router's own external address is itself behind another
    /// one, which is the case where port forwarding cannot work at all.
    pub carrier_nat: bool,
}

/// The public address, asked of the router rather than of a website.
///
/// Behind its own command because a public address is the one piece of this
/// screen somebody might not want on screen while streaming.
#[tauri::command]
pub async fn network_public_ip(
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Option<PublicAddress>> {
    let configured = instance_port(&state, id).await?;
    Ok(net::upnp::external_ip().await?.map(|address| PublicAddress {
        joinable: net::joinable(&address, configured),
        carrier_nat: net::upnp::is_carrier_nat(&address),
        address,
    }))
}

/// The port an instance is configured to use, read off disk.
async fn instance_port(state: &AppState, id: i64) -> AppResult<u16> {
    let row = instance::get(&state.db, id).await?;
    let path = row.path_buf();
    tokio::task::spawn_blocking(move || port::configured_port(&path))
        .await
        .map_err(|error| crate::error::AppError::Other(error.to_string()))
}

/// Whether the outside world can reach the server.
#[tauri::command]
pub async fn network_external_check(
    state: State<'_, AppState>,
    id: i64,
    host: String,
) -> AppResult<Reachability> {
    let configured = instance_port(&state, id).await?;
    net::external_reachability(&state.http, &host, configured).await
}

/// Whether a router that speaks UPnP answered at all.
#[tauri::command]
pub async fn network_upnp_available() -> AppResult<Option<String>> {
    net::upnp::discover().await
}

/// Asks the router to forward the port. Only ever called from a click.
#[tauri::command]
pub async fn network_upnp_map(
    state: State<'_, AppState>,
    id: i64,
    local_ip: String,
) -> AppResult<MappingResult> {
    let configured = instance_port(&state, id).await?;
    let parsed = local_ip.parse().map_err(|_| {
        crate::error::AppError::Other(format!("{local_ip} is not an address on this computer"))
    })?;

    net::upnp::map_port(parsed, configured).await
}

/// Removes the mapping again.
#[tauri::command]
pub async fn network_upnp_unmap(state: State<'_, AppState>, id: i64) -> AppResult<MappingResult> {
    let configured = instance_port(&state, id).await?;
    net::upnp::unmap_port(configured).await
}

//! LAN discovery and component distribution. Live web/RPC access requires a separate opt-in.
use std::{
    collections::BTreeMap,
    io::Write,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Path as UrlPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::runtime::{MAX_COMPONENT_BYTES, Runtime, SharedPlugin};

pub const SERVICE_TYPE: &str = "_hyperlocal._tcp.local.";
const MAX_CATALOG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub protocol: u32,
    pub name: String,
    pub plugins: Vec<SharedPlugin>,
}

#[derive(Debug, Serialize)]
pub struct Peer {
    pub service: String,
    pub name: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Discovery {
    pub peers: Vec<Peer>,
    pub warnings: Vec<String>,
}

struct Mdns(ServiceDaemon);
impl Mdns {
    fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("cannot start mDNS")?;
        let guard = Self(daemon);
        guard.0.disable_interface(IfKind::IPv6)?;
        // A remote machine must never be offered this machine's loopback address.
        guard.0.disable_interface(IfKind::LoopbackV4)?;
        Ok(guard)
    }
}
impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Browse only while requested, without advertising this machine or executing plugins.
pub fn discover(wait_ms: u64) -> Result<Discovery> {
    ensure!(
        (100..=10_000).contains(&wait_ms),
        "wait_ms must be between 100 and 10000"
    );
    let daemon = Mdns::new()?;
    let monitor = daemon.0.monitor()?;
    let events = daemon.0.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let mut peers = BTreeMap::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(remaining) else {
            break;
        };
        match event {
            ServiceEvent::ServiceResolved(info)
                if info.get_property_val_str("protocol") == Some("1") =>
            {
                let mut urls = info
                    .get_addresses_v4()
                    .into_iter()
                    .filter(|ip| allowed_ip(*ip) && !ip.is_loopback())
                    .map(|ip| format!("http://{ip}:{}/", info.get_port()))
                    .collect::<Vec<_>>();
                urls.sort();
                if !urls.is_empty() {
                    let name = info
                        .get_property_val_str("name")
                        .unwrap_or(info.get_fullname())
                        .to_owned();
                    peers.insert(
                        info.get_fullname().to_owned(),
                        Peer {
                            service: info.get_fullname().to_owned(),
                            name,
                            urls,
                        },
                    );
                }
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                peers.remove(&name);
            }
            _ => {}
        }
    }
    let warnings = monitor
        .try_iter()
        .filter_map(|event| match event {
            DaemonEvent::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();
    Ok(Discovery {
        peers: peers.into_values().collect(),
        warnings,
    })
}

#[derive(Clone)]
struct ShareState {
    runtime: Runtime,
    name: String,
    live: bool,
}

/// Binary sharing is the default; live web/function endpoints require explicit opt-in.
pub struct ShareServer {
    pub port: u16,
    pub name: String,
    daemon: Option<Mdns>,
    task: JoinHandle<()>,
}

impl ShareServer {
    /// `advertise=false` permits deterministic HTTP testing without relying on multicast routing.
    pub async fn start(runtime: Runtime, name: String, port: u16, advertise: bool) -> Result<Self> {
        Self::start_inner(runtime, name, port, advertise, false).await
    }

    /// Explicitly expose the web and function interfaces of connected plugins.
    pub async fn start_live(
        runtime: Runtime,
        name: String,
        port: u16,
        advertise: bool,
    ) -> Result<Self> {
        Self::start_inner(runtime, name, port, advertise, true).await
    }

    async fn start_inner(
        runtime: Runtime,
        name: String,
        port: u16,
        advertise: bool,
        live: bool,
    ) -> Result<Self> {
        ensure!(
            !name.is_empty() && name.len() <= 48 && !name.chars().any(char::is_control),
            "peer name must be 1–48 bytes without control characters"
        );
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .await
            .context("cannot bind LAN sharing listener")?;
        let port = listener.local_addr()?.port();
        let daemon = if advertise {
            let daemon = Mdns::new()?;
            let hostname = format!("hyperlocal-{}-{port}.local.", std::process::id());
            let instance = format!("{name}-{port}");
            let properties = [("protocol", "1"), ("name", name.as_str())];
            let service = ServiceInfo::new(
                SERVICE_TYPE,
                &instance,
                &hostname,
                "",
                port,
                &properties[..],
            )?
            .enable_addr_auto();
            daemon
                .0
                .register(service)
                .context("cannot advertise LAN sharing service")?;
            Some(daemon)
        } else {
            None
        };
        let mut router = Router::new()
            .route("/v1/catalog", get(catalog))
            .route("/v1/components/{digest}", get(component))
            .with_state(ShareState {
                runtime: runtime.clone(),
                name: name.clone(),
                live,
            });
        if live {
            router = router.merge(crate::remote::routes(runtime, name.clone()));
        }
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                eprintln!("peer sharing server failed: {error}");
            }
        });
        Ok(Self {
            port,
            name,
            daemon,
            task,
        })
    }
}
impl Drop for ShareServer {
    fn drop(&mut self) {
        self.task.abort();
        // Stop advertising before releasing the daemon; UI listeners have independent lifetimes.
        self.daemon.take();
    }
}

async fn catalog(State(state): State<ShareState>) -> Response {
    match state.runtime.shared_catalog() {
        Ok(mut plugins) => {
            for plugin in &mut plugins {
                plugin.web_available &= state.live;
                plugin.calls_available = state.live;
            }
            let catalog = Catalog {
                protocol: 1,
                name: state.name,
                plugins,
            };
            match serde_json::to_vec(&catalog) {
                Ok(bytes)
                    if bytes.len() as u64 <= MAX_CATALOG_BYTES && catalog.plugins.len() <= 256 =>
                {
                    (
                        [
                            (header::CONTENT_TYPE, "application/json"),
                            (header::CACHE_CONTROL, "no-store"),
                        ],
                        bytes,
                    )
                        .into_response()
                }
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Catalog exceeds distribution limits",
                )
                    .into_response(),
            }
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn component(State(state): State<ShareState>, UrlPath(digest): UrlPath<String>) -> Response {
    if !valid_digest(&digest) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match state.runtime.shared_component(&digest) {
        Ok(Some(bytes)) => Response::builder()
            .header(header::CONTENT_TYPE, "application/wasm")
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .body(Body::from(bytes.to_vec()))
            .unwrap(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn allowed_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_link_local()
}
pub(crate) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub(crate) fn peer_url(input: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(input).context("invalid peer URL")?;
    ensure!(
        url.scheme() == "http" && url.username().is_empty() && url.password().is_none(),
        "peer URL must use plain HTTP without credentials"
    );
    let ip = url
        .host_str()
        .context("peer URL needs an IPv4 address")?
        .parse::<Ipv4Addr>()
        .context("peer URL must use a numeric IPv4 address from discovery")?;
    ensure!(
        allowed_ip(ip),
        "peer address must be private, link-local or loopback"
    );
    ensure!(
        url.path() == "/" && url.query().is_none() && url.fragment().is_none(),
        "peer URL must be a base URL without path, query or fragment"
    );
    ensure!(
        url.port_or_known_default() != Some(0),
        "peer port cannot be zero"
    );
    Ok(url)
}

#[derive(Clone)]
pub struct PeerClient {
    pub(crate) client: reqwest::Client,
    directory: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct Download {
    pub plugin_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub path: PathBuf,
}

impl PeerClient {
    pub fn new(directory: PathBuf) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { client, directory })
    }

    pub(crate) async fn fetch(&self, url: reqwest::Url, limit: u64) -> Result<Vec<u8>> {
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .context("cannot reach peer")?;
        ensure!(
            response.status().is_success(),
            "peer returned HTTP {}",
            response.status()
        );
        if let Some(size) = response.content_length() {
            ensure!(size <= limit, "peer response exceeds size limit");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                (bytes.len() + chunk.len()) as u64 <= limit,
                "peer response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub async fn catalog(&self, base: &str) -> Result<Catalog> {
        let bytes = self
            .fetch(peer_url(base)?.join("v1/catalog")?, MAX_CATALOG_BYTES)
            .await?;
        let catalog: Catalog =
            serde_json::from_slice(&bytes).context("peer returned an invalid catalog")?;
        ensure!(
            catalog.protocol == 1,
            "unsupported peer protocol {}",
            catalog.protocol
        );
        ensure!(
            catalog.name.len() <= 128 && catalog.plugins.len() <= 256,
            "catalog exceeds entry limits"
        );
        let mut ids = std::collections::BTreeSet::new();
        for plugin in &catalog.plugins {
            ensure!(
                !plugin.id.is_empty()
                    && plugin.id.len() <= 128
                    && plugin
                        .id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                "invalid catalog plugin id"
            );
            ensure!(ids.insert(&plugin.id), "duplicate catalog plugin id");
            ensure!(
                valid_digest(&plugin.sha256)
                    && plugin.size_bytes > 0
                    && plugin.size_bytes <= MAX_COMPONENT_BYTES,
                "invalid catalog component digest or size"
            );
        }
        Ok(catalog)
    }

    /// Download the exact digest selected from a catalog. Never instantiate the received component.
    pub async fn download(&self, base: &str, plugin_id: &str, digest: &str) -> Result<Download> {
        ensure!(
            valid_digest(digest),
            "sha256 must be 64 lowercase hexadecimal characters"
        );
        let catalog = self.catalog(base).await?;
        let plugin = catalog
            .plugins
            .iter()
            .find(|p| p.id == plugin_id)
            .context("plugin is not in the peer catalog")?;
        ensure!(
            plugin.sha256 == digest,
            "peer plugin changed since inspection; list_peer_plugins again before downloading"
        );
        let bytes = self
            .fetch(
                peer_url(base)?.join(&format!("v1/components/{digest}"))?,
                MAX_COMPONENT_BYTES,
            )
            .await?;
        ensure!(
            bytes.len() as u64 == plugin.size_bytes,
            "component size differs from catalog"
        );
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == digest,
            "component SHA-256 mismatch"
        );
        ensure!(
            wasmparser::Parser::is_component(&bytes),
            "peer artifact is not a binary WebAssembly component"
        );
        wasmparser::Validator::new()
            .validate_all(&bytes)
            .context("peer returned an invalid component")?;
        let directory = self.directory.clone();
        let digest_owned = digest.to_owned();
        let size_bytes = bytes.len() as u64;
        let path =
            tokio::task::spawn_blocking(move || store_download(&directory, &digest_owned, &bytes))
                .await??;
        Ok(Download {
            plugin_id: plugin_id.to_owned(),
            sha256: digest.to_owned(),
            size_bytes,
            path,
        })
    }
}

fn store_download(directory: &Path, digest: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(directory).context("cannot create plugin download directory")?;
    let path = directory.join(format!("{digest}.wasm"));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let file = std::fs::File::open(&path)?;
            use std::io::Read;
            let mut existing = Vec::new();
            file.take(MAX_COMPONENT_BYTES + 1)
                .read_to_end(&mut existing)?;
            ensure!(
                existing.len() as u64 <= MAX_COMPONENT_BYTES
                    && format!("{:x}", Sha256::digest(&existing)) == digest,
                "cached component is corrupt; refusing to overwrite it"
            );
        }
        Err(error) => bail!("cannot store downloaded component: {}", error.error),
    }
    path.canonicalize()
        .context("cannot resolve downloaded component path")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_external_urls_and_arbitrary_paths() {
        for value in [
            "https://192.168.1.2/",
            "http://example.com/",
            "http://8.8.8.8/",
            "http://127.0.0.1/secret",
            "http://user:pass@127.0.0.1/",
            "http://127.0.0.1/?x=1",
        ] {
            assert!(peer_url(value).is_err(), "{value}");
        }
        assert!(peer_url("http://192.168.1.2:49152/").is_ok());
    }
}

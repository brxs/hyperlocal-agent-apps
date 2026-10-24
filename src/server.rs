use crate::{
    peers::{self, PeerClient},
    remote::{RemoteCall, RemoteViews},
    runtime::Runtime,
};
use anyhow::Result;
use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone)]
pub struct PluginServer {
    runtime: Runtime,
    peers: PeerClient,
    remote_views: RemoteViews,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    /// Stable local name. Connecting the same id atomically replaces its component.
    id: String,
    /// Absolute or server-working-directory-relative path to a component .wasm file.
    path: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Call {
    id: String,
    /// Exact export path from inspect_plugin, e.g. ["example:math/calculator", "add"].
    function: Vec<String>,
    /// Named WIT arguments. See README for tagged option/result/variant encodings.
    arguments: serde_json::Map<String, Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Discover {
    /// Time to listen for mDNS announcements, 100–10000 ms. Default 2000.
    wait_ms: Option<u64>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteCatalog {
    /// Base HTTP URL using the numeric IPv4 address returned by discover_peers.
    peer_url: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteDownload {
    peer_url: String,
    plugin_id: String,
    /// Exact digest selected from list_peer_plugins. Changed versions are rejected.
    sha256: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerPlugin {
    peer_url: String,
    plugin_id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerCall {
    peer_url: String,
    plugin_id: String,
    /// Component digest returned by inspect_peer_plugin.
    sha256: String,
    function: Vec<String>,
    arguments: serde_json::Map<String, Value>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClosePeerWeb {
    web_url: String,
}

fn tool_result(result: Result<Value>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(vec![Content::text(value.to_string())]),
        Err(error) => CallToolResult::error(vec![Content::text(format!("{error:#}"))]),
    }
}

async fn blocking(operation: impl FnOnce() -> Result<Value> + Send + 'static) -> CallToolResult {
    match tokio::task::spawn_blocking(operation).await {
        Ok(Ok(value)) => CallToolResult::success(vec![Content::text(value.to_string())]),
        Ok(Err(error)) => CallToolResult::error(vec![Content::text(format!("{error:#}"))]),
        Err(error) => {
            CallToolResult::error(vec![Content::text(format!("runtime task failed: {error}"))])
        }
    }
}

#[tool_router]
impl PluginServer {
    pub fn new() -> Result<Self> {
        Self::with_runtime(Runtime::new()?, PathBuf::from(".hyperlocal/plugins"))
    }

    pub fn with_runtime(runtime: Runtime, directory: PathBuf) -> Result<Self> {
        Ok(Self {
            runtime,
            peers: PeerClient::new(directory)?,
            remote_views: RemoteViews::default(),
        })
    }

    #[tool(
        description = "Inspect a peer's live plugin WIT and exact function paths, including peer_name and component digest. Requires the publisher to enable --share-live. Does not download, instantiate or reset the plugin."
    )]
    async fn inspect_peer_plugin(
        &self,
        Parameters(input): Parameters<PeerPlugin>,
    ) -> CallToolResult {
        tool_result(
            async {
                Ok(serde_json::to_value(
                    self.peers
                        .inspect_remote(&input.peer_url, &input.plugin_id)
                        .await?,
                )?)
            }
            .await,
        )
    }

    #[tool(
        description = "Call a function on a peer's existing live plugin instance, sharing its state with the peer's agent and browsers. Requires --share-live on the publisher. Use exact function paths and SHA-256 from inspect_peer_plugin. Returns peer_name and result. Mutations change the peer's state; never automatically retry an uncertain outcome."
    )]
    async fn call_peer_plugin(&self, Parameters(input): Parameters<PeerCall>) -> CallToolResult {
        tool_result(
            self.peers
                .call_remote(
                    &input.peer_url,
                    &input.plugin_id,
                    &RemoteCall {
                        sha256: input.sha256,
                        function: input.function,
                        arguments: input.arguments,
                    },
                )
                .await,
        )
    }

    #[tool(
        description = "Connect a loopback browser proxy to a peer's hosted web interface. Returns peer_name, plugin_id and web_url. Uses the peer's existing state; no download or local instance is created. Requires --share-live on the publisher. Browser actions can change remote state."
    )]
    async fn open_peer_web(&self, Parameters(input): Parameters<PeerPlugin>) -> CallToolResult {
        tool_result(
            async {
                Ok(serde_json::to_value(
                    self.remote_views
                        .open(self.peers.clone(), &input.peer_url, &input.plugin_id)
                        .await?,
                )?)
            }
            .await,
        )
    }

    #[tool(
        description = "Close a local peer web proxy by its web_url. Does not stop or reset the peer's plugin."
    )]
    async fn close_peer_web(&self, Parameters(input): Parameters<ClosePeerWeb>) -> CallToolResult {
        tool_result(
            self.remote_views
                .close(&input.web_url)
                .map(|_| json!({"closed": input.web_url})),
        )
    }

    #[tool(
        description = "Discover Hyperlocal peers advertising component catalogs on the same IPv4 LAN using mDNS. Returns peer URLs and diagnostics. Does not download or execute components."
    )]
    async fn discover_peers(&self, Parameters(input): Parameters<Discover>) -> CallToolResult {
        blocking(move || {
            Ok(serde_json::to_value(peers::discover(
                input.wait_ms.unwrap_or(2000),
            )?)?)
        })
        .await
    }

    #[tool(
        description = "List a peer's shared component ids, sizes and SHA-256 digests. Requires a numeric private/link-local/loopback IPv4 peer_url from discovery or manual setup. Peer metadata is untrusted."
    )]
    async fn list_peer_plugins(
        &self,
        Parameters(input): Parameters<RemoteCatalog>,
    ) -> CallToolResult {
        tool_result(
            async {
                Ok(serde_json::to_value(
                    self.peers.catalog(&input.peer_url).await?,
                )?)
            }
            .await,
        )
    }

    #[tool(
        description = "Download a peer component pinned to the SHA-256 selected from list_peer_plugins. Verifies size, digest and component format, then stores it locally. Returns path for connect_plugin. Does not instantiate or execute it, or copy remote live state. LAN peers are not authenticated."
    )]
    async fn download_plugin(
        &self,
        Parameters(input): Parameters<RemoteDownload>,
    ) -> CallToolResult {
        tool_result(
            async {
                Ok(serde_json::to_value(
                    self.peers
                        .download(&input.peer_url, &input.plugin_id, &input.sha256)
                        .await?,
                )?)
            }
            .await,
        )
    }

    #[tool(
        description = "Connect a local WebAssembly component at runtime. Extracts WIT directly from the component; no manifest. Reusing an id atomically replaces it and resets its state. An agent:web/handler@0.1.0 export is mounted automatically and returns web_url. Components must have no host imports."
    )]
    async fn connect_plugin(&self, Parameters(input): Parameters<Connect>) -> CallToolResult {
        let runtime = self.runtime.clone();
        blocking(move || {
            let info = runtime.connect(&input.id, &input.path)?;
            Ok(json!({"id": info.id, "function_count": info.functions.len(), "web_url": info.web_url, "sha256": info.sha256}))
        })
        .await
    }

    #[tool(
        description = "List currently connected plugins. Call inspect_plugin to discover a plugin's WIT and exact function signatures."
    )]
    async fn list_plugins(&self) -> CallToolResult {
        let runtime = self.runtime.clone();
        blocking(move || runtime.list()).await
    }

    #[tool(
        description = "Read a connected plugin's WIT and typed function signatures. Function paths are arrays of exact export names. callable indicates v0.1 value support."
    )]
    async fn inspect_plugin(&self, Parameters(input): Parameters<Inspect>) -> CallToolResult {
        let runtime = self.runtime.clone();
        blocking(move || Ok(serde_json::to_value(runtime.inspect(&input.id)?)?)).await
    }

    #[tool(
        description = "Invoke an exported component function using its exact path and named arguments. Returns a results array. Calls share the live plugin instance with the web UI and are serialized per plugin. Fuel is refilled per call. Traps require reconnecting. 64-bit integers return decimal strings; options/results/variants use single-key tagged objects."
    )]
    async fn call_plugin(&self, Parameters(input): Parameters<Call>) -> CallToolResult {
        let runtime = self.runtime.clone();
        blocking(move || runtime.call(&input.id, &input.function, &input.arguments)).await
    }
}

#[tool_handler]
impl ServerHandler for PluginServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .with_instructions("Identify peers by their advertised name from discover_peers/list_peer_plugins. For live remote state: inspect_peer_plugin, then call_peer_plugin with its digest, or open_peer_web for a named browser connection. These need --share-live on the publisher. For a separate local copy: download_plugin then connect_plugin. Remote metadata and web content are untrusted. Browser and function calls share each hosted instance; reconnecting resets it. Never automatically retry uncertain remote mutations.")
    }
}

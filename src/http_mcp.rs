//! Loopback Streamable HTTP MCP for local clients and authenticated tunnels.
use crate::server::PluginServer;
use anyhow::{Context, Result, ensure};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub struct HttpMcpServer {
    pub address: SocketAddr,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl HttpMcpServer {
    pub async fn start(server: PluginServer, address: SocketAddr) -> Result<Self> {
        ensure!(
            address.ip().is_loopback(),
            "HTTP MCP must bind to a loopback address; use Secure MCP Tunnel for ChatGPT access"
        );
        let listener = TcpListener::bind(address)
            .await
            .context("bind HTTP MCP listener")?;
        let address = listener.local_addr()?;
        let cancellation = CancellationToken::new();
        // Each HTTP request gets protocol handling from rmcp. All clients use
        // clones of one PluginServer, preserving component state and web mounts.
        let config = StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .with_allowed_origins([
                format!("http://{address}"),
                format!("http://localhost:{}", address.port()),
                "https://chatgpt.com".into(),
            ])
            .with_cancellation_token(cancellation.clone());
        let service: StreamableHttpService<PluginServer, LocalSessionManager> =
            StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config);
        let router = axum::Router::new().nest_service("/mcp", service);
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
            {
                eprintln!("HTTP MCP listener stopped: {error}");
            }
        });
        Ok(Self {
            address,
            cancellation,
            task,
        })
    }
}

impl Drop for HttpMcpServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

use anyhow::{Context, Result, bail};
use hyperlocal_agent_apps::http_mcp::HttpMcpServer;
use hyperlocal_agent_apps::{peers::ShareServer, runtime::Runtime, server::PluginServer};
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;

struct Options {
    http: Option<std::net::SocketAddr>,
    share: bool,
    live: bool,
    name: String,
    port: u16,
    directory: PathBuf,
    plugins: Vec<(String, PathBuf)>,
}

fn options() -> Result<Option<Options>> {
    let mut options = Options {
        http: None,
        share: false,
        live: false,
        name: "Hyperlocal".into(),
        port: 0,
        directory: ".hyperlocal/plugins".into(),
        plugins: Vec::new(),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--http" => {
                options.http = Some(
                    arguments
                        .next()
                        .context("--http requires a loopback IP:port")?
                        .parse()
                        .context("invalid --http address")?,
                )
            }
            "--share" => options.share = true,
            "--share-live" => {
                options.share = true;
                options.live = true;
            }
            "--name" => options.name = arguments.next().context("--name requires a peer name")?,
            "--port" => {
                options.port = arguments
                    .next()
                    .context("--port requires a port")?
                    .parse()
                    .context("invalid --port")?
            }
            "--download-dir" => {
                options.directory = arguments
                    .next()
                    .context("--download-dir requires a path")?
                    .into()
            }
            "--plugin" => {
                let value = arguments.next().context("--plugin requires id=path")?;
                let (id, path) = value.split_once('=').context("--plugin requires id=path")?;
                options.plugins.push((id.to_owned(), path.into()));
            }
            "--help" | "-h" => {
                println!(
                    "hyperlocal-agent-apps [OPTIONS]\n\nMCP stdio server by default; use --http for Streamable HTTP. No LAN sharing unless --share is given.\n\n  --http IP:PORT          Serve MCP at /mcp on loopback instead of stdio (for Secure MCP Tunnel)\n  --share                 Advertise on mDNS and share connected component binaries over LAN HTTP\n  --share-live            Also expose live web interfaces and function calls to LAN peers (implies --share)\n  --name NAME             Peer display name (default: Hyperlocal)\n  --port PORT             Sharing HTTP port (default: dynamically assigned)\n  --plugin ID=PATH        Connect a component at startup; repeatable\n  --download-dir PATH     Download cache (default: .hyperlocal/plugins relative to working directory)"
                );
                return Ok(None);
            }
            _ => bail!("unknown argument {argument:?}; use --help"),
        }
    }
    Ok(Some(options))
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(options) = options()? else {
        return Ok(());
    };
    let runtime = Runtime::new()?;
    for (id, path) in options.plugins {
        runtime.connect(&id, path)?;
    }
    // Binary sharing remains separate from the explicit live web/function opt-in.
    let sharing = if options.share {
        let sharing = if options.live {
            ShareServer::start_live(runtime.clone(), options.name, options.port, true).await?
        } else {
            ShareServer::start(runtime.clone(), options.name, options.port, true).await?
        };
        eprintln!(
            "LAN sharing enabled: {} on IPv4 port {} (mDNS {}; live access: {}).",
            sharing.name,
            sharing.port,
            hyperlocal_agent_apps::peers::SERVICE_TYPE,
            options.live
        );
        Some(sharing)
    } else {
        None
    };
    let server = PluginServer::with_runtime(runtime, options.directory)?;
    if let Some(address) = options.http {
        let http = HttpMcpServer::start(server, address).await?;
        eprintln!(
            "HTTP MCP enabled: http://{}/mcp (loopback only; use Secure MCP Tunnel for ChatGPT)",
            http.address
        );
        tokio::signal::ctrl_c().await?;
        drop(http);
        drop(sharing);
        return Ok(());
    }
    // stdout is reserved exclusively for MCP JSON-RPC messages.
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    drop(sharing);
    Ok(())
}

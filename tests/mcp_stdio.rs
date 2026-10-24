use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Client {
    child: Child,
    stdin: ChildStdin,
    responses: mpsc::Receiver<Value>,
    id: u64,
}
impl Client {
    fn new() -> Self {
        Self::with_args(&[])
    }
    fn with_args(arguments: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hyperlocal-agent-apps"))
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = serde_json::from_str(&line.unwrap())
                    .expect("stdout must contain only JSON-RPC");
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            responses,
            id: 0,
        }
    }
    fn send(&mut self, value: Value) {
        writeln!(self.stdin, "{value}").unwrap();
        self.stdin.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}));
        loop {
            let response = self
                .responses
                .recv_timeout(Duration::from_secs(30))
                .expect("MCP response timed out");
            if response.get("id") == Some(&json!(self.id)) {
                assert!(response.get("error").is_none(), "{response}");
                return response["result"].clone();
            }
        }
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn content(result: &Value) -> Value {
    assert_ne!(result["isError"], json!(true), "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn stdio_session_discovers_connects_and_invokes_plugins_without_restart() {
    let mut client = Client::new();
    let initialized = client.request("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "integration-test", "version": "1"}}));
    assert!(initialized["capabilities"]["tools"].is_object());
    assert_eq!(initialized["serverInfo"]["name"], "hyperlocal-agent-apps");
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let tools = client.request("tools/list", json!({}));
    assert_eq!(tools["tools"].as_array().unwrap().len(), 11);
    assert_eq!(
        content(&client.tool("list_plugins", json!({})))["plugins"],
        json!([])
    );
    for id in ["one", "two"] {
        let result = client.tool("connect_plugin", json!({"id": id, "path": format!("{}/fixtures/calculator.wat", env!("CARGO_MANIFEST_DIR"))}));
        assert_eq!(content(&result)["id"], id);
    }
    let inspected = content(&client.tool("inspect_plugin", json!({"id": "two"})));
    assert!(
        inspected["wit"]
            .as_str()
            .unwrap()
            .contains("interface calculator")
    );
    let args = json!({"id": "two", "function": ["example:math/calculator@0.1.0", "add"], "arguments": {"a": 20, "b": 22}});
    assert_eq!(
        content(&client.tool("call_plugin", args.clone()))["results"],
        json!([42])
    );
    let failed = client.tool(
        "call_plugin",
        json!({"id": "two", "function": ["trap"], "arguments": {}}),
    );
    assert_eq!(failed["isError"], true);
    let faulted = client.tool("call_plugin", args.clone());
    assert_eq!(faulted["isError"], true);
    assert!(
        faulted["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("faulted")
    );
    content(&client.tool("connect_plugin", json!({"id": "two", "path": format!("{}/fixtures/calculator.wat", env!("CARGO_MANIFEST_DIR"))})));

    assert_eq!(
        content(&client.tool("call_plugin", args))["results"],
        json!([42])
    );
    assert_eq!(
        content(&client.tool("list_plugins", json!({})))["plugins"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(client.request("tools/list", json!({})), tools);
    let web = content(&client.tool("connect_plugin", json!({"id": "web", "path": format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR"))})));
    let url = web["web_url"]
        .as_str()
        .expect("web export must be mounted at connection time");
    assert!(url.starts_with("http://127.0.0.1:"));
    assert_eq!(
        content(&client.tool("inspect_plugin", json!({"id": "web"})))["web_url"],
        url
    );
    assert_eq!(
        browser_json(url, "GET", "/api/info")["component"],
        "web-demo"
    );
    let agent = |name: &str, arguments: Value| json!({"id": "web", "function": ["example:counter/state@0.1.0", name], "arguments": arguments});
    assert_eq!(
        content(&client.tool("call_plugin", agent("increment", json!({"amount": 41}))))["results"],
        json!([41])
    );
    assert_eq!(browser_json(url, "GET", "/api/counter"), 41);
    assert_eq!(browser_json(url, "POST", "/api/increment"), 42);
    assert_eq!(
        content(&client.tool("call_plugin", agent("get", json!({}))))["results"],
        json!([42])
    );
    assert_eq!(browser_json(url, "POST", "/api/reset"), 0);
    assert_eq!(
        content(&client.tool("call_plugin", agent("get", json!({}))))["results"],
        json!([0])
    );
}

fn browser_json(url: &str, method: &str, path: &str) -> Value {
    let authority = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut socket = std::net::TcpStream::connect(authority).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(socket, "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut socket, &mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_downloads_a_peer_component_then_connects_its_own_instance() {
    use hyperlocal_agent_apps::{peers::ShareServer, runtime::Runtime};
    let source = Runtime::new().unwrap();
    source
        .connect(
            "counter",
            format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    source
        .call(
            "counter",
            &["example:counter/state@0.1.0".into(), "increment".into()],
            json!({"amount": 41}).as_object().unwrap(),
        )
        .unwrap();
    let sharing = ShareServer::start(source.clone(), "mcp-peer".into(), 0, false)
        .await
        .unwrap();
    let peer_url = format!("http://127.0.0.1:{}/", sharing.port);
    let downloads = tempfile::tempdir().unwrap();
    let mut client = Client::with_args(&["--download-dir", downloads.path().to_str().unwrap()]);
    client.request("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "peer-test", "version": "1"}}));
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let catalog = content(&client.tool("list_peer_plugins", json!({"peer_url": peer_url})));
    let entry = &catalog["plugins"][0];
    let downloaded = content(&client.tool(
        "download_plugin",
        json!({"peer_url": peer_url, "plugin_id": entry["id"], "sha256": entry["sha256"]}),
    ));
    assert!(std::path::Path::new(downloaded["path"].as_str().unwrap()).is_file());
    assert_eq!(
        content(&client.tool("list_plugins", json!({})))["plugins"],
        json!([])
    );
    let connected = content(&client.tool(
        "connect_plugin",
        json!({"id": "copy", "path": downloaded["path"]}),
    ));
    assert_eq!(connected["sha256"], entry["sha256"]);
    let state = content(&client.tool(
        "call_plugin",
        json!({"id": "copy", "function": ["example:counter/state@0.1.0", "get"], "arguments": {}}),
    ));
    assert_eq!(state["results"], json!([0]));
    let source_state = source
        .call(
            "counter",
            &["example:counter/state@0.1.0".into(), "get".into()],
            json!({}).as_object().unwrap(),
        )
        .unwrap();
    assert_eq!(source_state["results"], json!([41]));
    // The downloaded web interface mounts locally just like a local plugin.
    assert_eq!(
        browser_json(
            connected["web_url"].as_str().unwrap(),
            "GET",
            "/api/counter"
        ),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_opens_named_peer_web_and_calls_its_live_functions_without_downloads() {
    use hyperlocal_agent_apps::{peers::ShareServer, runtime::Runtime};
    let source = Runtime::new().unwrap();
    source
        .connect(
            "counter",
            format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    let sharing = ShareServer::start_live(source.clone(), "Office PC".into(), 0, false)
        .await
        .unwrap();
    let peer_url = format!("http://127.0.0.1:{}/", sharing.port);
    let files = tempfile::tempdir().unwrap();
    let cache = files.path().join("unused");
    let mut client = Client::with_args(&["--download-dir", cache.to_str().unwrap()]);
    client.request("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "live-peer-test", "version": "1"}}));
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let identity = json!({"peer_url": peer_url, "plugin_id": "counter"});
    let info = content(&client.tool("inspect_peer_plugin", identity.clone()));
    assert_eq!(info["peer_name"], "Office PC");
    let remote_call = |name: &str, args: Value| json!({"peer_url": peer_url, "plugin_id": "counter", "sha256": info["sha256"], "function": ["example:counter/state@0.1.0", name], "arguments": args});
    let result = content(&client.tool(
        "call_peer_plugin",
        remote_call("increment", json!({"amount": 41})),
    ));
    assert_eq!(result["peer_name"], "Office PC");
    assert_eq!(result["result"]["results"], json!([41]));
    let view = content(&client.tool("open_peer_web", identity));
    assert_eq!(view["peer_name"], "Office PC");
    let web_url = view["web_url"].as_str().unwrap();
    assert_eq!(
        browser_json(web_url, "GET", "/.hyperlocal/connection")["peer_name"],
        "Office PC"
    );
    assert_eq!(browser_json(web_url, "GET", "/api/counter"), 41);
    assert_eq!(browser_json(web_url, "POST", "/api/increment"), 42);
    assert_eq!(
        content(&client.tool("call_peer_plugin", remote_call("get", json!({}))))["result"]["results"],
        json!([42])
    );
    assert_eq!(
        content(&client.tool("list_plugins", json!({})))["plugins"],
        json!([])
    );
    assert!(!cache.exists());
    content(&client.tool("close_peer_web", json!({"web_url": web_url})));
    assert_eq!(
        source
            .call(
                "counter",
                &["example:counter/state@0.1.0".into(), "get".into()],
                json!({}).as_object().unwrap()
            )
            .unwrap()["results"],
        json!([42])
    );
}

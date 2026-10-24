use hyperlocal_agent_apps::{
    http_mcp::HttpMcpServer, peers::ShareServer, runtime::Runtime, server::PluginServer,
};
use serde_json::{Value, json};

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
async fn rpc(url: &str, method: &str, params: Value) -> Value {
    let response = http()
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert!(value.get("error").is_none(), "{value}");
    value["result"].clone()
}
async fn call(url: &str, name: &str, arguments: Value) -> Value {
    let value = rpc(
        url,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await;
    assert_ne!(value["isError"], true, "{value}");
    serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap()
}
async fn start(runtime: Runtime) -> (HttpMcpServer, tempfile::TempDir, String) {
    let cache = tempfile::tempdir().unwrap();
    let server = PluginServer::with_runtime(runtime, cache.path().to_owned()).unwrap();
    let endpoint = HttpMcpServer::start(server, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let url = format!("http://{}/mcp", endpoint.address);
    (endpoint, cache, url)
}

#[tokio::test(flavor = "multi_thread")]
async fn http_initialization_and_independent_requests_share_plugin_state() {
    let runtime = Runtime::new().unwrap();
    let (_endpoint, _cache, url) = start(runtime.clone()).await;
    let initialized = rpc(&url, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"chatgpt-test","version":"1"}})).await;
    assert_eq!(initialized["serverInfo"]["name"], "hyperlocal-agent-apps");
    let tools = rpc(&url, "tools/list", json!({})).await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 11);
    call(&url, "connect_plugin", json!({"id":"restaurant","path":format!("{}/fixtures/pizzeria.wasm", env!("CARGO_MANIFEST_DIR"))})).await;
    let function = ["restaurant:pizzeria/service@0.1.0", "place-order"];
    let receipt = call(&url, "call_plugin", json!({"id":"restaurant","function":function,"arguments":{"pizza-id":"hawaii","quantity":1,"table":7,"request-id":"http-hawaii-001"}})).await;
    assert_eq!(receipt["results"][0]["ok"]["id"], 1);
    let orders = call(&url, "call_plugin", json!({"id":"restaurant","function":["restaurant:pizzeria/service@0.1.0","get-open-orders"],"arguments":{}})).await;
    assert_eq!(orders["results"][0].as_array().unwrap().len(), 1);
    assert_eq!(
        runtime.list().unwrap()["plugins"].as_array().unwrap().len(),
        1
    );
    let notification = http()
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .body(json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(notification.status(), 202);
}

#[tokio::test(flavor = "multi_thread")]
async fn http_guest_can_inspect_and_order_on_lan_peer() {
    let restaurant = Runtime::new().unwrap();
    restaurant
        .connect(
            "restaurant",
            format!("{}/fixtures/pizzeria.wasm", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    let publisher = ShareServer::start_live(restaurant.clone(), "Forno Pizza".into(), 0, false)
        .await
        .unwrap();
    let peer_url = format!("http://127.0.0.1:{}/", publisher.port);
    let (_endpoint, _cache, url) = start(Runtime::new().unwrap()).await;
    let info = call(
        &url,
        "inspect_peer_plugin",
        json!({"peer_url":peer_url,"plugin_id":"restaurant"}),
    )
    .await;
    let receipt = call(&url, "call_peer_plugin", json!({"peer_url":peer_url,"plugin_id":"restaurant","sha256":info["sha256"],"function":["restaurant:pizzeria/service@0.1.0","place-order"],"arguments":{"pizza-id":"checkpoint-gpt","quantity":1,"table":12,"request-id":"chatgpt-demo-001"}})).await;
    assert_eq!(
        receipt["result"]["results"][0]["ok"]["pizza-name"],
        "Checkpoint GPT"
    );
    let orders = restaurant
        .call(
            "restaurant",
            &[
                "restaurant:pizzeria/service@0.1.0".into(),
                "get-open-orders".into(),
            ],
            &serde_json::Map::new(),
        )
        .unwrap();
    assert_eq!(orders["results"][0][0]["table"], 12);
}

#[tokio::test]
async fn http_rejects_nonlocal_binding_rebinding_and_foreign_origins() {
    let cache = tempfile::tempdir().unwrap();
    let server =
        PluginServer::with_runtime(Runtime::new().unwrap(), cache.path().to_owned()).unwrap();
    assert!(
        HttpMcpServer::start(server, "0.0.0.0:0".parse().unwrap())
            .await
            .is_err()
    );
    let (endpoint, _cache, url) = start(Runtime::new().unwrap()).await;
    for (header, value) in [
        ("host", "unrelated.example"),
        ("origin", "https://unrelated.example"),
    ] {
        let response = http()
            .post(&url)
            .header(header, value)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    let address = endpoint.address;
    drop(endpoint);
    tokio::task::yield_now().await;
    assert!(
        http()
            .get(format!("http://{address}/mcp"))
            .send()
            .await
            .is_err()
    );
}

use hyperlocal_agent_apps::{
    peers::{PeerClient, ShareServer},
    remote::{RemoteCall, RemoteViews},
    runtime::Runtime,
};
use serde_json::{Value, json};

fn fixture() -> String {
    format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR"))
}
fn call(digest: &str, name: &str, args: Value) -> RemoteCall {
    RemoteCall {
        sha256: digest.into(),
        function: vec!["example:counter/state@0.1.0".into(), name.into()],
        arguments: args.as_object().unwrap().clone(),
    }
}
fn local_count(source: &Runtime) -> Value {
    source
        .call(
            "counter",
            &["example:counter/state@0.1.0".into(), "get".into()],
            json!({}).as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .clone()
}
async fn browser(url: &str, method: &str, path: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .request(
            method.parse().unwrap(),
            format!("{}{path}", url.trim_end_matches('/')),
        )
        .header("Origin", url.trim_end_matches('/'))
        .send()
        .await
        .unwrap()
}
async fn browser_json(url: &str, method: &str, path: &str) -> Value {
    let response = browser(url, method, path).await;
    assert!(response.status().is_success(), "{}", response.status());
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_functions_and_proxied_browser_use_the_named_peers_existing_state() {
    let source = Runtime::new().unwrap();
    source.connect("counter", fixture()).unwrap();
    let sharing = ShareServer::start_live(source.clone(), "Studio Mac".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().join("unused")).unwrap();
    let catalog = client.catalog(&base).await.unwrap();
    assert_eq!(catalog.name, "Studio Mac");
    assert!(catalog.plugins[0].web_available && catalog.plugins[0].calls_available);
    let info = client.inspect_remote(&base, "counter").await.unwrap();
    assert_eq!(info.peer_name, "Studio Mac");
    assert!(info.wit.contains("increment"));
    assert!(
        info.functions
            .iter()
            .any(|f| f.path.last().map(String::as_str) == Some("increment"))
    );
    let result = client
        .call_remote(
            &base,
            "counter",
            &call(&info.sha256, "increment", json!({"amount": 41})),
        )
        .await
        .unwrap();
    assert_eq!(result["peer_name"], "Studio Mac");
    assert_eq!(result["result"]["results"], json!([41]));
    let views = RemoteViews::default();
    let view = views.open(client.clone(), &base, "counter").await.unwrap();
    assert_eq!(view.peer_name, "Studio Mac");
    assert_eq!(
        views
            .open(client.clone(), &base, "counter")
            .await
            .unwrap()
            .web_url,
        view.web_url
    );
    assert_eq!(browser_json(&view.web_url, "GET", "/api/counter").await, 41);
    assert_eq!(
        browser_json(&view.web_url, "POST", "/api/increment").await,
        42
    );
    assert_eq!(local_count(&source), 42);
    let remote = client
        .call_remote(&base, "counter", &call(&info.sha256, "get", json!({})))
        .await
        .unwrap();
    assert_eq!(remote["result"]["results"], json!([42]));
    let metadata = browser_json(&view.web_url, "GET", "/.hyperlocal/connection").await;
    assert_eq!(metadata["peer_name"], "Studio Mac");
    assert_eq!(metadata["plugin_id"], "counter");
    let page = browser(&view.web_url, "GET", "/").await;
    assert!(page.text().await.unwrap().contains("Two ways in."));
    let js = browser(&view.web_url, "GET", "/app.js").await;
    assert!(js.text().await.unwrap().contains("peer.peer_name"));
    assert_eq!(
        browser(&view.web_url, "GET", "/api/path?q=hello%20world")
            .await
            .text()
            .await
            .unwrap(),
        "/api/path?q=hello%20world"
    );
    assert!(!cache.path().join("unused").exists());
    views.close(&view.web_url).unwrap();
    assert_eq!(local_count(&source), 42);
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_sharing_does_not_enable_live_access() {
    let source = Runtime::new().unwrap();
    source.connect("counter", fixture()).unwrap();
    let sharing = ShareServer::start(source.clone(), "Download only".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let client = PeerClient::new(tempfile::tempdir().unwrap().path().to_owned()).unwrap();
    let catalog = client.catalog(&base).await.unwrap();
    assert!(!catalog.plugins[0].web_available && !catalog.plugins[0].calls_available);
    assert!(client.inspect_remote(&base, "counter").await.is_err());
    assert!(
        RemoteViews::default()
            .open(client.clone(), &base, "counter")
            .await
            .is_err()
    );
    let digest = &catalog.plugins[0].sha256;
    assert!(
        client
            .call_remote(
                &base,
                "counter",
                &call(digest, "increment", json!({"amount": 1}))
            )
            .await
            .is_err()
    );
    assert_eq!(local_count(&source), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_versions_and_invalid_arguments_do_not_mutate_remote_state() {
    let source = Runtime::new().unwrap();
    source.connect("counter", fixture()).unwrap();
    let sharing = ShareServer::start_live(source.clone(), "Versioned".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().to_owned()).unwrap();
    let info = client.inspect_remote(&base, "counter").await.unwrap();
    assert!(
        client
            .call_remote(
                &base,
                "counter",
                &call(&"0".repeat(64), "increment", json!({"amount": 10}))
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("version changed")
    );
    assert!(
        client
            .call_remote(
                &base,
                "counter",
                &call(&info.sha256, "increment", json!({"amount": -1}))
            )
            .await
            .is_err()
    );
    assert_eq!(local_count(&source), 0);
    let views = RemoteViews::default();
    let view = views.open(client.clone(), &base, "counter").await.unwrap();
    let changed = cache.path().join("changed.wat");
    std::fs::write(
        &changed,
        include_str!("../fixtures/web-demo.wat").replace("i32.const 200", "i32.const 201"),
    )
    .unwrap();
    source.connect("counter", changed).unwrap();
    assert!(
        client
            .call_remote(
                &base,
                "counter",
                &call(&info.sha256, "increment", json!({"amount": 10}))
            )
            .await
            .is_err()
    );
    assert_eq!(
        browser(&view.web_url, "GET", "/api/counter").await.status(),
        500
    );
    assert_eq!(local_count(&source), 0);
    let updated = views.open(client, &base, "counter").await.unwrap();
    assert_ne!(updated.sha256, view.sha256);
    assert_eq!(
        browser_json(&updated.web_url, "GET", "/api/counter").await,
        0
    );
    views.close(&updated.web_url).unwrap();
    assert_eq!(local_count(&source), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_cannot_call_machine_gateway_directly() {
    let source = Runtime::new().unwrap();
    let info = source.connect("counter", fixture()).unwrap();
    let sharing = ShareServer::start_live(source.clone(), "Guarded".into(), 0, false)
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{}/v1/plugins/counter/call", sharing.port);
    let body =
        serde_json::to_vec(&call(&info.sha256, "increment", json!({"amount": 100}))).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .header("origin", "http://example.com")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .header("sec-fetch-site", "cross-site")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = client
        .post(&url)
        .header("content-type", "text/plain")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 415);
    assert_eq!(local_count(&source), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_does_not_forward_ambient_credentials_or_accept_remote_cookies() {
    use axum::{
        Router,
        body::to_bytes,
        extract::{Request, State},
        routing::{get, post},
    };
    use std::sync::{Arc, Mutex};
    let captured = Arc::new(Mutex::new(None::<Value>));
    let inspection = json!({"peer_name": "Guest laptop", "plugin_id": "test", "sha256": "a".repeat(64), "wit": "", "functions": [], "web_available": true}).to_string();
    let app = Router::new()
        .route("/v1/plugins/test", get(move || { let inspection = inspection.clone(); async { inspection } }))
        .route("/v1/plugins/test/web", post(|State(captured): State<Arc<Mutex<Option<Value>>>>, request: Request| async move {
            assert!(!request.headers().contains_key("origin"));
            let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
            *captured.lock().unwrap() = Some(serde_json::from_slice(&bytes).unwrap());
            json!({"status": 200, "headers": [["content-type", "text/plain"], ["set-cookie", "remote=bad"]], "body": [111, 107]}).to_string()
        })).with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().to_owned()).unwrap();
    let views = RemoteViews::default();
    let view = views.open(client, &base, "test").await.unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&view.web_url)
        .header("cookie", "local=secret")
        .header("authorization", "Bearer local-secret")
        .header("origin", view.web_url.trim_end_matches('/'))
        .header("referer", &view.web_url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(response.text().await.unwrap(), "ok");
    let captured = captured.lock().unwrap();
    let headers = captured.as_ref().unwrap()["request"]["headers"]
        .as_array()
        .unwrap();
    for header in headers {
        assert!(
            !["cookie", "authorization", "origin", "referer", "host"]
                .contains(&header[0].as_str().unwrap())
        );
    }
    service.abort();
}

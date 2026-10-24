use hyperlocal_agent_apps::{
    peers::{Catalog, PeerClient, ShareServer},
    runtime::{Runtime, SharedPlugin},
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn fixture() -> String {
    format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR"))
}
fn count(runtime: &Runtime, id: &str, name: &str, args: serde_json::Value) -> serde_json::Value {
    runtime
        .call(
            id,
            &["example:counter/state@0.1.0".into(), name.into()],
            args.as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_immutable_components_and_instantiates_independent_state() {
    let source = Runtime::new().unwrap();
    let files = tempfile::tempdir().unwrap();
    let original = files.path().join("source.wat");
    std::fs::copy(fixture(), &original).unwrap();
    source.connect("counter", &original).unwrap();
    count(&source, "counter", "increment", json!({"amount": 41}));
    let sharing = ShareServer::start(source.clone(), "source".into(), 0, false)
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = files.path().join("downloads");
    let client = PeerClient::new(cache.clone()).unwrap();
    let catalog = client.catalog(&url).await.unwrap();
    assert_eq!(catalog.plugins.len(), 1);
    let entry = &catalog.plugins[0];
    assert_eq!(entry.id, "counter");
    // The network catalog never includes machine paths, browser URLs or mutable state.
    let encoded = serde_json::to_string(&catalog).unwrap();
    assert!(!encoded.contains("source.wat"));
    assert!(!encoded.contains("web_url"));
    // Source file changes cannot change the component bytes captured on connection.
    std::fs::write(&original, "this is no longer a component").unwrap();
    let downloaded = client
        .download(&url, &entry.id, &entry.sha256)
        .await
        .unwrap();
    let bytes = std::fs::read(&downloaded.path).unwrap();
    assert!(wasmparser::Parser::is_component(&bytes));
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), entry.sha256);
    assert_eq!(
        client
            .download(&url, &entry.id, &entry.sha256)
            .await
            .unwrap()
            .path,
        downloaded.path
    );
    let local = Runtime::new().unwrap();
    assert_eq!(local.list().unwrap()["plugins"], json!([]));
    local.connect("downloaded", &downloaded.path).unwrap();
    assert_eq!(count(&local, "downloaded", "get", json!({})), 0);
    assert_eq!(count(&source, "counter", "get", json!({})), 41);
    assert_eq!(
        count(&local, "downloaded", "increment", json!({"amount": 7})),
        7
    );
    assert_eq!(count(&source, "counter", "get", json!({})), 41);
    // Corrupt cache entries are never silently trusted or overwritten.
    std::fs::write(&downloaded.path, "corrupt").unwrap();
    assert!(
        client
            .download(&url, &entry.id, &entry.sha256)
            .await
            .unwrap_err()
            .to_string()
            .contains("corrupt")
    );
    // A peer changing a plugin requires selecting a new digest explicitly.
    let updated = files.path().join("updated.wat");
    std::fs::write(
        &updated,
        include_str!("../fixtures/web-demo.wat").replace("i32.const 200", "i32.const 201"),
    )
    .unwrap();
    source.connect("counter", updated).unwrap();
    assert!(
        client
            .download(&url, &entry.id, &entry.sha256)
            .await
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    // Only the catalog and component endpoints are mounted on the LAN listener.
    let response = reqwest::get(format!("{url}api/counter")).await.unwrap();
    assert_eq!(response.status(), 404);
    drop(sharing);
}

async fn malicious_peer(catalog: Catalog, bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{
        Router,
        http::{StatusCode, header},
        routing::get,
    };
    let encoded = serde_json::to_vec(&catalog).unwrap();
    let app = Router::new()
        .route(
            "/v1/catalog",
            get(move || {
                let encoded = encoded.clone();
                async { ([(header::CONTENT_TYPE, "application/json")], encoded) }
            }),
        )
        .route(
            "/v1/components/{digest}",
            get(move || {
                let bytes = bytes.clone();
                async { (StatusCode::OK, bytes) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn rejects_corruption_and_non_component_artifacts_before_writing_files() {
    let component = wat::parse_str("(component)").unwrap();
    let digest = format!("{:x}", Sha256::digest(&component));
    let catalog = Catalog {
        protocol: 1,
        name: "bad".into(),
        plugins: vec![SharedPlugin {
            id: "test".into(),
            sha256: digest.clone(),
            size_bytes: component.len() as u64,
            function_count: 0,
            web_available: false,
            calls_available: false,
        }],
    };
    let mut corrupt = component.clone();
    corrupt[0] ^= 1;
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let client = PeerClient::new(cache.clone()).unwrap();
    let (url, server) = malicious_peer(catalog, corrupt).await;
    assert!(
        client
            .download(&url, "test", &digest)
            .await
            .unwrap_err()
            .to_string()
            .contains("SHA-256 mismatch")
    );
    assert!(!cache.exists());
    server.abort();
    let module = wat::parse_str("(module)").unwrap();
    let digest = format!("{:x}", Sha256::digest(&module));
    let catalog = Catalog {
        protocol: 1,
        name: "bad".into(),
        plugins: vec![SharedPlugin {
            id: "test".into(),
            sha256: digest.clone(),
            size_bytes: module.len() as u64,
            function_count: 0,
            web_available: false,
            calls_available: false,
        }],
    };
    let (url, server) = malicious_peer(catalog, module).await;
    assert!(
        client
            .download(&url, "test", &digest)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a binary WebAssembly component")
    );
    assert!(!cache.exists());
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn downloading_does_not_execute_component_start_functions() {
    let bytes = wat::parse_str("(component (core module $m (func $start unreachable) (start $start)) (core instance (instantiate $m)))").unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let catalog = Catalog {
        protocol: 1,
        name: "test".into(),
        plugins: vec![SharedPlugin {
            id: "start-trap".into(),
            sha256: digest.clone(),
            size_bytes: bytes.len() as u64,
            function_count: 0,
            web_available: false,
            calls_available: false,
        }],
    };
    let (url, server) = malicious_peer(catalog, bytes).await;
    let dir = tempfile::tempdir().unwrap();
    let client = PeerClient::new(dir.path().to_owned()).unwrap();
    let downloaded = client.download(&url, "start-trap", &digest).await.unwrap();
    // Validation and download succeed, whereas explicit instantiation reaches the trap.
    let local = Runtime::new().unwrap();
    assert!(local.connect("trap", downloaded.path).is_err());
    assert_eq!(local.list().unwrap()["plugins"], json!([]));
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a multicast-capable LAN interface; run explicitly for mDNS verification"]
async fn mdns_discovers_a_live_advertised_peer() {
    let source = Runtime::new().unwrap();
    let sharing = ShareServer::start(source, "mdns-test".into(), 0, true)
        .await
        .unwrap();
    let report = tokio::task::spawn_blocking(|| hyperlocal_agent_apps::peers::discover(5000))
        .await
        .unwrap()
        .unwrap();
    assert!(
        report
            .peers
            .iter()
            .all(|p| p.urls.iter().all(|url| !url.starts_with("http://127.")))
    );
    assert!(
        report.peers.iter().any(|p| p.name == "mdns-test"
            && p.urls
                .iter()
                .any(|url| url.contains(&format!(":{}/", sharing.port)))),
        "{report:?}"
    );
}

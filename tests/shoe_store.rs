use hyperlocal_agent_apps::{
    peers::{PeerClient, ShareServer},
    remote::{RemoteCall, RemoteViews},
    runtime::Runtime,
};
use serde_json::{Value, json};
const SERVICE: &str = "berlin:shoes/inventory@0.1.0";
fn fixture() -> String {
    format!("{}/fixtures/shoe-store.wasm", env!("CARGO_MANIFEST_DIR"))
}
fn invoke(runtime: &Runtime, name: &str, args: Value) -> Value {
    runtime
        .call(
            "shoes",
            &[SERVICE.into(), name.into()],
            args.as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .clone()
}
fn reservation(request: &str) -> Value {
    json!({"product-id":"ringbahn-runner","size":41,"request-id":request})
}
fn available(runtime: &Runtime) -> Value {
    invoke(runtime, "get-catalog", json!({}))["products"][0]["sizes"][5]["available"].clone()
}

#[test]
fn stock_is_reserved_once_cancelled_and_released_without_resurrecting_a_retry() {
    let runtime = Runtime::new().unwrap();
    runtime.connect("shoes", fixture()).unwrap();
    assert_eq!(
        invoke(&runtime, "get-catalog", json!({}))["products"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(available(&runtime), 1);
    let receipt = invoke(&runtime, "reserve", reservation("agent-pickup-001"));
    assert_eq!(receipt["ok"]["status"], "reserved");
    assert_eq!(available(&runtime), 0);
    assert_eq!(
        invoke(&runtime, "reserve", reservation("agent-pickup-001")),
        receipt
    );
    assert_eq!(
        invoke(&runtime, "reserve", reservation("agent-pickup-002")),
        json!({"err":"out-of-stock"})
    );
    let cancelled = invoke(&runtime, "cancel", json!({"id":1}));
    assert_eq!(cancelled["ok"]["status"], "cancelled");
    assert_eq!(available(&runtime), 1);
    assert_eq!(invoke(&runtime, "cancel", json!({"id":1})), cancelled);
    assert_eq!(
        invoke(&runtime, "reserve", reservation("agent-pickup-001")),
        cancelled
    );
    assert_eq!(available(&runtime), 1);
    assert_eq!(
        invoke(&runtime, "reserve", reservation("agent-pickup-002"))["ok"]["id"],
        2
    );
    assert_eq!(
        invoke(&runtime, "get-reservation", json!({"id":999})),
        json!({"none":null})
    );
    assert_eq!(
        invoke(&runtime, "cancel", json!({"id":999})),
        json!({"err":"unknown-reservation"})
    );
    assert_eq!(
        invoke(
            &runtime,
            "reserve",
            json!({"product-id":"ringbahn-runner","size":42,"request-id":"agent-pickup-002"})
        ),
        json!({"err":"request-id-conflict"})
    );
    for (args, error) in [
        (
            json!({"product-id":"missing","size":41,"request-id":"valid-request-01"}),
            "unknown-product",
        ),
        (
            json!({"product-id":"ringbahn-runner","size":99,"request-id":"valid-request-02"}),
            "invalid-size",
        ),
        (
            json!({"product-id":"ringbahn-runner","size":41,"request-id":"short"}),
            "invalid-request-id",
        ),
    ] {
        assert_eq!(invoke(&runtime, "reserve", args), json!({"err":error}));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_and_peer_share_stock_and_concurrent_guests_cannot_oversell() {
    let runtime = Runtime::new().unwrap();
    let info = runtime.connect("shoes", fixture()).unwrap();
    let publisher = ShareServer::start_live(runtime.clone(), "Kiez Kicks".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", publisher.port);
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().to_owned()).unwrap();
    assert_eq!(client.catalog(&base).await.unwrap().name, "Kiez Kicks");
    let request = |id: &str| RemoteCall {
        sha256: info.sha256.clone(),
        function: vec![SERVICE.into(), "reserve".into()],
        arguments: reservation(id).as_object().unwrap().clone(),
    };
    let one = request("concurrent-shoe-01");
    let two = request("concurrent-shoe-02");
    let (one, two) = tokio::join!(
        client.call_remote(&base, "shoes", &one),
        client.call_remote(&base, "shoes", &two)
    );
    let values = [one.unwrap(), two.unwrap()];
    assert_eq!(
        values
            .iter()
            .filter(|v| v["result"]["results"][0].get("ok").is_some())
            .count(),
        1
    );
    assert_eq!(
        values
            .iter()
            .filter(|v| v["result"]["results"][0]["err"] == "out-of-stock")
            .count(),
        1
    );
    let views = RemoteViews::default();
    let web = views.open(client, &base, "shoes").await.unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["", "style.css", "app.js", "shoe.svg"] {
        assert_eq!(
            http.get(format!("{}{path}", web.web_url))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let holds: Value = serde_json::from_slice(
        &http
            .get(format!("{}api/reservations", web.web_url))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(holds["reservations"].as_array().unwrap().len(), 1);
    let cancelled = http
        .post(format!("{}api/cancel", web.web_url))
        .header("content-type", "application/json")
        .body(json!({"id":1}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(cancelled.status(), 200);
    assert_eq!(available(&runtime), 1);
    let body = json!({"product_id":"ringbahn-runner","size":41,"request_id":"browser-pickup-001"})
        .to_string();
    for _ in 0..2 {
        assert_eq!(
            http.post(format!("{}api/reserve", web.web_url))
                .header("content-type", "application/json")
                .body(body.clone())
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert_eq!(
        invoke(&runtime, "get-reservations", json!({}))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(available(&runtime), 0);
    assert_eq!(
        http.post(format!("{}api/reserve", web.web_url))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.post(format!("{}api/cancel", web.web_url))
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
}

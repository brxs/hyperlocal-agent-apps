use hyperlocal_agent_apps::{
    peers::{PeerClient, ShareServer},
    remote::{RemoteCall, RemoteViews},
    runtime::Runtime,
};
use serde_json::{Value, json};

const SERVICE: &str = "restaurant:pizzeria/service@0.1.0";
fn fixture() -> String {
    format!("{}/fixtures/pizzeria.wasm", env!("CARGO_MANIFEST_DIR"))
}
fn invoke(runtime: &Runtime, name: &str, args: Value) -> Value {
    runtime
        .call(
            "restaurant",
            &[SERVICE.into(), name.into()],
            args.as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .clone()
}
fn order(pizza: &str, quantity: u32, table: u32, request: &str) -> Value {
    json!({"pizza-id": pizza, "quantity": quantity, "table": table, "request-id": request})
}
fn remote(digest: &str, name: &str, arguments: Value) -> RemoteCall {
    RemoteCall {
        sha256: digest.into(),
        function: vec![SERVICE.into(), name.into()],
        arguments: arguments.as_object().unwrap().clone(),
    }
}
async fn browser_json(url: &str, path: &str) -> Value {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}{path}", url.trim_end_matches('/')))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}

#[test]
fn menu_contains_hawaii_and_black_dough_berghain_and_exports_typed_service() {
    let runtime = Runtime::new().unwrap();
    let info = runtime.connect("restaurant", fixture()).unwrap();
    assert!(info.wit.contains("place-order"));
    assert!(info.web_url.is_some());
    let menu = invoke(&runtime, "get-menu", json!({}));
    assert_eq!(menu["restaurant"], "Forno Pizza");
    assert_eq!(menu["currency"], "EUR");
    let pizzas = menu["pizzas"].as_array().unwrap();
    assert_eq!(pizzas.len(), 8);
    let hawaii = pizzas.iter().find(|p| p["id"] == "hawaii").unwrap();
    assert_eq!(hawaii["price-cents"], 1350);
    assert!(
        hawaii["description"]
            .as_str()
            .unwrap()
            .contains("pineapple")
    );
    let berghain = pizzas.iter().find(|p| p["id"] == "berghain").unwrap();
    assert!(
        berghain["description"]
            .as_str()
            .unwrap()
            .contains("Black dough")
    );
}

#[test]
fn ordering_is_validated_and_idempotent_without_faulting_the_guest() {
    let runtime = Runtime::new().unwrap();
    runtime.connect("restaurant", fixture()).unwrap();
    let args = order("hawaii", 2, 7, "table-7-hawaii-001");
    let received = invoke(&runtime, "place-order", args.clone());
    assert_eq!(received["ok"]["id"], 1);
    assert_eq!(received["ok"]["pizza-name"], "Hawaii");
    assert_eq!(received["ok"]["total-cents"], 2700);
    assert_eq!(received["ok"]["status"], "received");
    assert_eq!(invoke(&runtime, "place-order", args), received);
    assert_eq!(
        invoke(&runtime, "get-order", json!({"id": 1}))["some"],
        received["ok"]
    );
    assert_eq!(
        invoke(&runtime, "get-order", json!({"id": 999})),
        json!({"none": null})
    );
    assert_eq!(
        invoke(
            &runtime,
            "place-order",
            order("hawaii", 1, 7, "table-7-hawaii-001")
        ),
        json!({"err": "request-id-conflict"})
    );
    for (args, error) in [
        (order("missing", 1, 7, "valid-request-01"), "unknown-pizza"),
        (
            order("hawaii", 0, 7, "valid-request-02"),
            "invalid-quantity",
        ),
        (
            order("hawaii", 6, 7, "valid-request-03"),
            "invalid-quantity",
        ),
        (order("hawaii", 1, 0, "valid-request-04"), "invalid-table"),
        (order("hawaii", 1, 41, "valid-request-05"), "invalid-table"),
        (order("hawaii", 1, 7, "short"), "invalid-request-id"),
    ] {
        assert_eq!(invoke(&runtime, "place-order", args), json!({"err": error}));
    }
    assert_eq!(
        invoke(&runtime, "get-table-orders", json!({"table": 7}))["ok"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        invoke(&runtime, "get-table-orders", json!({"table": 8})),
        json!({"ok": []})
    );
    assert_eq!(
        invoke(&runtime, "get-table-orders", json!({"table": 0})),
        json!({"err": "invalid-table"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_reads_menu_orders_hawaii_remotely_and_sees_receipt_on_restaurant_website() {
    let restaurant = Runtime::new().unwrap();
    restaurant.connect("restaurant", fixture()).unwrap();
    let sharing = ShareServer::start_live(restaurant.clone(), "Forno Pizza".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().join("unused")).unwrap();
    let catalog = client.catalog(&base).await.unwrap();
    assert_eq!(catalog.name, "Forno Pizza");
    let info = client.inspect_remote(&base, "restaurant").await.unwrap();
    let views = RemoteViews::default();
    let web = views
        .open(client.clone(), &base, "restaurant")
        .await
        .unwrap();
    let menu = client
        .call_remote(
            &base,
            "restaurant",
            &remote(&info.sha256, "get-menu", json!({})),
        )
        .await
        .unwrap();
    assert_eq!(menu["result"]["results"][0]["restaurant"], "Forno Pizza");
    assert_eq!(
        browser_json(&web.web_url, "/api/orders?table=7").await["orders"],
        json!([])
    );
    let args = order("hawaii", 1, 7, "wifi-table-7-hawaii-001");
    let receipt = client
        .call_remote(
            &base,
            "restaurant",
            &remote(&info.sha256, "place-order", args.clone()),
        )
        .await
        .unwrap();
    assert_eq!(receipt["peer_name"], "Forno Pizza");
    assert_eq!(receipt["result"]["results"][0]["ok"]["total-cents"], 1350);
    let repeated = client
        .call_remote(
            &base,
            "restaurant",
            &remote(&info.sha256, "place-order", args),
        )
        .await
        .unwrap();
    assert_eq!(repeated["result"], receipt["result"]);
    let table = browser_json(&web.web_url, "/api/orders?table=7").await;
    assert_eq!(table["orders"].as_array().unwrap().len(), 1);
    assert_eq!(table["orders"][0]["pizza_name"], "Hawaii");
    assert_eq!(table["orders"][0]["total_cents"], 1350);
    assert_eq!(
        invoke(&restaurant, "get-order", json!({"id": 1}))["some"]["pizza-name"],
        "Hawaii"
    );
    assert_eq!(
        browser_json(&web.web_url, "/api/orders?table=8").await["orders"],
        json!([])
    );
    assert!(!cache.path().join("unused").exists());
    // The menu is hosted inside WASM: root-relative CSS, JS and illustration load through the proxy.
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["/", "/style.css", "/app.js", "/pizza.svg"] {
        assert_eq!(
            http.get(format!("{}{path}", web.web_url.trim_end_matches('/')))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert_eq!(
        http.post(format!("{}api/orders", web.web_url))
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_orders_and_peer_orders_share_back_office_and_validate_requests() {
    let runtime = Runtime::new().unwrap();
    let info = runtime.connect("restaurant", fixture()).unwrap();
    let sharing = ShareServer::start_live(runtime.clone(), "Forno Pizza".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = tempfile::tempdir().unwrap();
    let peer = PeerClient::new(cache.path().to_owned()).unwrap();
    let views = RemoteViews::default();
    let view = views.open(peer.clone(), &base, "restaurant").await.unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let body =
        json!({"pizza_id":"hawaii","quantity":2,"table":7,"request_id":"browser-hawaii-001"});
    for _ in 0..2 {
        let response = http
            .post(format!("{}api/orders", view.web_url))
            .header("origin", view.web_url.trim_end_matches('/'))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let data: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!(data["order"]["id"], 1);
        assert_eq!(data["order"]["total_cents"], 2700);
    }
    for invalid in [
        json!({}),
        json!({"quantity":4294967297_u64,"table":7,"pizza_id":"hawaii","request_id":"invalid-quantity-01"}),
        json!({"quantity":1,"table":0,"pizza_id":"hawaii","request_id":"invalid-table-001"}),
    ] {
        assert_eq!(
            http.post(format!("{}api/orders", view.web_url))
                .header("content-type", "application/json")
                .body(invalid.to_string())
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        http.post(format!("{}api/orders", view.web_url))
            .header("origin", "https://unrelated.example")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let args = order("berghain", 1, 12, "agent-berghain-001");
    peer.call_remote(
        &base,
        "restaurant",
        &remote(&info.sha256, "place-order", args),
    )
    .await
    .unwrap();
    let open = browser_json(&view.web_url, "/api/open-orders").await;
    assert_eq!(open["orders"].as_array().unwrap().len(), 2);
    assert_eq!(open["orders"][1]["pizza_name"], "Berghain");
    assert_eq!(
        invoke(&runtime, "get-open-orders", json!({}))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for path in ["back-office", "back-office.js"] {
        assert_eq!(
            http.get(format!("{}{path}", view.web_url))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_retries_cannot_create_duplicate_orders_and_berghain_is_orderable() {
    let restaurant = Runtime::new().unwrap();
    let info = restaurant.connect("restaurant", fixture()).unwrap();
    let sharing = ShareServer::start_live(restaurant.clone(), "Forno Pizza".into(), 0, false)
        .await
        .unwrap();
    let base = format!("http://127.0.0.1:{}/", sharing.port);
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().to_owned()).unwrap();
    let request = remote(
        &info.sha256,
        "place-order",
        order("berghain", 1, 7, "concurrent-berghain-001"),
    );
    let (one, two) = tokio::join!(
        client.call_remote(&base, "restaurant", &request),
        client.call_remote(&base, "restaurant", &request)
    );
    let one = one.unwrap();
    assert_eq!(one["result"], two.unwrap()["result"]);
    assert_eq!(one["result"]["results"][0]["ok"]["pizza-name"], "Berghain");
    assert_eq!(one["result"]["results"][0]["ok"]["total-cents"], 1650);
    assert_eq!(
        invoke(&restaurant, "get-table-orders", json!({"table": 7}))["ok"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

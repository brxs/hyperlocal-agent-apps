use hyperlocal_agent_apps::{
    peers::{PeerClient, ShareServer},
    remote::{RemoteCall, RemoteViews},
    runtime::Runtime,
};
use serde_json::{Value, json};
const SERVICE: &str = "foodcourt:vendor/service@0.1.0";
const VENDORS: [(&str, &str, &str); 4] = [
    ("tacos", "Kiez Tacos", "al-pastor"),
    ("ramen", "Mitte Ramen", "miso-ramen"),
    ("spritz", "Spree Spritz", "spree-spritz"),
    ("taproom", "Kreuzberg Taproom", "kiez-pils"),
];
fn fixture(slug: &str) -> String {
    format!(
        "{}/fixtures/foodcourt-{slug}.wasm",
        env!("CARGO_MANIFEST_DIR")
    )
}
fn invoke(runtime: &Runtime, name: &str, args: Value) -> Value {
    runtime
        .call(
            "vendor",
            &[SERVICE.into(), name.into()],
            args.as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .clone()
}

#[test]
fn four_distinct_components_expose_vendor_menus_and_isolated_order_state() {
    let mut digests = std::collections::HashSet::new();
    for (slug, name, item) in VENDORS {
        let runtime = Runtime::new().unwrap();
        let info = runtime.connect("vendor", fixture(slug)).unwrap();
        assert!(info.wit.contains("foodcourt:vendor/service@0.1.0"));
        assert!(digests.insert(info.sha256));
        let menu = invoke(&runtime, "get-menu", json!({}));
        assert_eq!(menu["restaurant"], name);
        assert_eq!(
            menu["items"].as_array().unwrap().len(),
            if slug == "taproom" { 5 } else { 4 }
        );
        assert_eq!(menu["items"][0]["id"], item);
        assert_eq!(invoke(&runtime, "get-open-orders", json!({})), json!([]));
        let args =
            json!({"item-id":item,"quantity":2,"table":7,"request-id":"same-id-different-vendors"});
        let receipt = invoke(&runtime, "place-order", args.clone());
        assert_eq!(receipt["ok"]["id"], 1);
        assert_eq!(receipt["ok"]["item-id"], item);
        assert_eq!(invoke(&runtime, "place-order", args), receipt);
        assert_eq!(
            receipt["ok"]["total-cents"].as_u64().unwrap(),
            menu["items"][0]["price-cents"].as_u64().unwrap() * 2
        );
        assert_eq!(
            invoke(
                &runtime,
                "place-order",
                json!({"item-id":"not-on-menu","quantity":1,"table":7,"request-id":"invalid-menu-item-001"})
            ),
            json!({"err":"unknown-item"})
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_reaches_four_independent_peers_and_browser_orders_stay_with_the_right_vendor() {
    let cache = tempfile::tempdir().unwrap();
    let client = PeerClient::new(cache.path().to_owned()).unwrap();
    let views = RemoteViews::default();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut peers = Vec::new();
    for (slug, name, item) in VENDORS {
        let runtime = Runtime::new().unwrap();
        runtime.connect("vendor", fixture(slug)).unwrap();
        let publisher = ShareServer::start_live(runtime.clone(), name.into(), 0, false)
            .await
            .unwrap();
        let url = format!("http://127.0.0.1:{}/", publisher.port);
        assert_eq!(client.catalog(&url).await.unwrap().name, name);
        let info = client.inspect_remote(&url, "vendor").await.unwrap();
        let web = views.open(client.clone(), &url, "vendor").await.unwrap();
        let menu: Value = serde_json::from_slice(
            &http
                .get(format!("{}api/menu", web.web_url))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(menu["restaurant"], name);
        assert_eq!(menu["items"][0]["id"], item);
        for path in ["", "app.js", "style.css", "back-office", "back-office.js"] {
            assert_eq!(
                http.get(format!("{}{path}", web.web_url))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
        peers.push((runtime, publisher, url, info, web, item));
    }
    let first = &peers[0];
    let body =
        json!({"item_id":first.5,"quantity":1,"table":12,"request_id":"foodcourt-browser-001"});
    let response = http
        .post(format!("{}api/orders", first.4.web_url))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        invoke(&first.0, "get-open-orders", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for peer in peers.iter().skip(1) {
        assert_eq!(invoke(&peer.0, "get-open-orders", json!({})), json!([]));
    }
    let second = &peers[1];
    let request = RemoteCall {
        sha256: second.3.sha256.clone(),
        function: vec![SERVICE.into(), "place-order".into()],
        arguments:
            json!({"item-id":second.5,"quantity":1,"table":12,"request-id":"foodcourt-agent-001"})
                .as_object()
                .unwrap()
                .clone(),
    };
    let receipt = client
        .call_remote(&second.2, "vendor", &request)
        .await
        .unwrap();
    assert_eq!(receipt["peer_name"], "Mitte Ramen");
    assert_eq!(receipt["result"]["results"][0]["ok"]["id"], 1);
    let orders: Value = serde_json::from_slice(
        &http
            .get(format!("{}api/open-orders", second.4.web_url))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(orders["orders"][0]["item_name"], "Miso Ramen");
}

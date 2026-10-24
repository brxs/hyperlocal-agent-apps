use hyperlocal_agent_apps::runtime::Runtime;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

fn fixture() -> String {
    format!("{}/fixtures/web-demo.wat", env!("CARGO_MANIFEST_DIR"))
}

struct Response {
    status: u16,
    headers: String,
    body: Vec<u8>,
}
fn request(url: &str, method: &str, path: &str, headers: &str, body: &[u8]) -> Response {
    let authority = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    let mut stream = TcpStream::connect_timeout(
        &authority.parse::<SocketAddr>().unwrap(),
        Duration::from_secs(5),
    )
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n", body.len()).unwrap();
    stream.write_all(body).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let split = bytes.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    Response {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    }
}

#[test]
fn mounts_assets_and_api_and_keeps_url_across_replacement() {
    let runtime = Runtime::new().unwrap();
    let info = runtime.connect("web", fixture()).unwrap();
    let url = info.web_url.unwrap();
    assert_eq!(runtime.inspect("web").unwrap().web_url.as_ref(), Some(&url));
    assert_eq!(runtime.list().unwrap()["plugins"][0]["web_url"], url);
    let home = request(&url, "GET", "/", "", &[]);
    assert_eq!(home.status, 200);
    assert!(home.headers.contains("text/html"));
    assert!(
        String::from_utf8(home.body)
            .unwrap()
            .contains("Two ways in.")
    );
    let js = request(&url, "GET", "/app.js", "", &[]);
    assert!(js.headers.contains("text/javascript"));
    assert!(String::from_utf8(js.body).unwrap().contains("fetch(path"));
    let api = request(&url, "GET", "/api/info", "", &[]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&api.body).unwrap()["mounted"],
        true
    );
    let body = br#"{"message":"hello"}"#;
    let echo = request(
        &url,
        "POST",
        "/api/echo",
        &format!(
            "Origin: {}\r\nContent-Type: application/json\r\n",
            url.trim_end_matches('/')
        ),
        body,
    );
    assert_eq!(echo.status, 200);
    assert_eq!(echo.body, body);
    let path = "/api/path?message=hello%20world";
    assert_eq!(request(&url, "GET", path, "", &[]).body, path.as_bytes());
    assert_eq!(request(&url, "GET", "/missing", "", &[]).status, 404);
    assert!(request(&url, "HEAD", "/", "", &[]).body.is_empty());
    // Mounts live independently from MCP calls and the returned metadata.
    let replacement = tempfile::tempdir().unwrap();
    let file = replacement.path().join("replacement.wasm");
    std::fs::write(
        &file,
        wat::parse_str(
            include_str!("../fixtures/web-demo.wat").replace("i32.const 200", "i32.const 201"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        runtime.connect("web", file).unwrap().web_url.as_ref(),
        Some(&url)
    );
    assert_eq!(request(&url, "GET", "/", "", &[]).status, 201);
    // Each plugin has its own origin and root-relative assets.
    let other_url = runtime
        .connect("other", fixture())
        .unwrap()
        .web_url
        .unwrap();
    assert_ne!(other_url, url);
    assert_eq!(request(&other_url, "GET", "/", "", &[]).status, 200);
}

#[test]
fn rejects_wrong_web_contract_without_replacing_a_live_mount() {
    let runtime = Runtime::new().unwrap();
    let url = runtime.connect("web", fixture()).unwrap().web_url.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("invalid.wat");
    std::fs::write(
        &file,
        include_str!("../fixtures/web-demo.wat")
            .replace("(field \"status\" u16)", "(field \"status\" u32)"),
    )
    .unwrap();
    assert!(
        runtime
            .connect("web", file)
            .unwrap_err()
            .to_string()
            .contains("must match wit/web.wit")
    );
    assert_eq!(request(&url, "GET", "/", "", &[]).status, 200);
}

#[test]
fn blocks_cross_origin_requests_and_recovers_from_guest_failures() {
    let runtime = Runtime::new().unwrap();
    let url = runtime.connect("web", fixture()).unwrap().web_url.unwrap();
    assert_eq!(
        request(
            &url,
            "POST",
            "/api/echo",
            "Origin: https://example.com\r\n",
            b"{}"
        )
        .status,
        403
    );
    assert_eq!(
        request(
            &url,
            "GET",
            "/api/info",
            "Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: cors\r\n",
            &[]
        )
        .status,
        403
    );
    assert_eq!(
        request(
            &url,
            "GET",
            "/",
            "Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\n",
            &[]
        )
        .status,
        200
    );
    for path in ["/trap", "/spin"] {
        runtime.connect("web", fixture()).unwrap();
        assert_eq!(request(&url, "GET", path, "", &[]).status, 500);
        assert_eq!(request(&url, "GET", "/", "", &[]).status, 500);
    }
    runtime.connect("web", fixture()).unwrap();
    assert_eq!(request(&url, "GET", "/", "", &[]).status, 200);
    assert_eq!(
        request(&url, "POST", "/api/echo", "", &vec![b'x'; 1024 * 1024 + 1]).status,
        413
    );
    // Non-web plugins remain available as normal MCP components.
    let math = runtime
        .connect(
            "math",
            format!("{}/fixtures/calculator.wat", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    assert_eq!(math.web_url, None);
    assert_eq!(
        runtime
            .call(
                "math",
                &["example:math/calculator@0.1.0".into(), "add".into()],
                json!({"a": 1, "b": 2}).as_object().unwrap()
            )
            .unwrap()["results"],
        json!([3])
    );
}

#[test]
fn runtime_drop_unmounts_listeners() {
    let runtime = Runtime::new().unwrap();
    let url = runtime.connect("web", fixture()).unwrap().web_url.unwrap();
    assert_eq!(request(&url, "GET", "/", "", &[]).status, 200);
    let addr = url.strip_prefix("http://").unwrap().trim_end_matches('/');
    drop(runtime);
    let mut closed = false;
    for _ in 0..100 {
        if TcpStream::connect(addr).is_err() {
            closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(closed, "mount must close when the runtime is dropped");
}

fn counter(runtime: &Runtime, id: &str, name: &str, arguments: serde_json::Value) -> u64 {
    runtime
        .call(
            id,
            &["example:counter/state@0.1.0".into(), name.into()],
            arguments.as_object().unwrap(),
        )
        .unwrap()["results"][0]
        .as_u64()
        .unwrap()
}
fn browser_count(url: &str, method: &str, path: &str) -> u64 {
    let response = request(url, method, path, "", &[]);
    assert_eq!(response.status, 200);
    serde_json::from_slice::<u64>(&response.body).unwrap()
}

#[test]
fn agent_and_browser_share_guest_state_and_reconnection_resets_it() {
    let runtime = Runtime::new().unwrap();
    let url = runtime
        .connect("counter", fixture())
        .unwrap()
        .web_url
        .unwrap();
    assert_eq!(counter(&runtime, "counter", "get", json!({})), 0);
    assert_eq!(
        counter(&runtime, "counter", "increment", json!({"amount": 41})),
        41
    );
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 41);
    assert_eq!(browser_count(&url, "POST", "/api/increment"), 42);
    assert_eq!(counter(&runtime, "counter", "get", json!({})), 42);
    // Reading and reloading the page must not reinstantiate or reset the guest.
    assert_eq!(request(&url, "GET", "/", "", &[]).status, 200);
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 42);
    assert_eq!(request(&url, "GET", "/api/increment", "", &[]).status, 405);
    assert_eq!(counter(&runtime, "counter", "get", json!({})), 42);
    assert_eq!(browser_count(&url, "POST", "/api/reset"), 0);
    assert_eq!(counter(&runtime, "counter", "get", json!({})), 0);
    counter(&runtime, "counter", "increment", json!({"amount": 7}));
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.wat");
    std::fs::write(&bad, "(component (core module $m (func $start unreachable) (start $start)) (core instance (instantiate $m)))").unwrap();
    assert!(runtime.connect("counter", bad).is_err());
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 7);
    let other = runtime
        .connect("other", fixture())
        .unwrap()
        .web_url
        .unwrap();
    assert_eq!(browser_count(&other, "GET", "/api/counter"), 0);
    assert_eq!(
        runtime
            .connect("counter", fixture())
            .unwrap()
            .web_url
            .as_ref(),
        Some(&url)
    );
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 0);
}

#[test]
fn concurrent_agent_and_browser_updates_do_not_lose_increments() {
    let runtime = Runtime::new().unwrap();
    let url = runtime
        .connect("counter", fixture())
        .unwrap()
        .web_url
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads = (0..8)
        .map(|i| {
            let runtime = runtime.clone();
            let url = url.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..20 {
                    if i % 2 == 0 {
                        counter(&runtime, "counter", "increment", json!({"amount": 1}));
                    } else {
                        browser_count(&url, "POST", "/api/increment");
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(counter(&runtime, "counter", "get", json!({})), 160);
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 160);
}

#[test]
fn repeated_http_calls_reclaim_request_memory_without_resetting_state() {
    let runtime = Runtime::new().unwrap();
    let url = runtime
        .connect("counter", fixture())
        .unwrap()
        .web_url
        .unwrap();
    counter(&runtime, "counter", "increment", json!({"amount": 9}));
    let body = vec![b'x'; 256 * 1024];
    // Cumulatively more input than the guest's fixed 2 MiB memory.
    for _ in 0..10 {
        let response = request(&url, "POST", "/api/echo", "", &body);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, body);
    }
    assert_eq!(browser_count(&url, "GET", "/api/counter"), 9);
}

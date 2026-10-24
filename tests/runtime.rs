use hyperlocal_agent_apps::runtime::Runtime;
use serde_json::{Value, json};

fn fixture() -> String {
    format!("{}/fixtures/calculator.wat", env!("CARGO_MANIFEST_DIR"))
}
fn arguments(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn path(name: &str) -> Vec<String> {
    vec!["example:math/calculator@0.1.0".into(), name.into()]
}

#[test]
fn discovers_and_invokes_without_static_bindings() {
    let runtime = Runtime::new().unwrap();
    assert_eq!(runtime.list().unwrap()["plugins"], json!([]));
    let info = runtime.connect("math", fixture()).unwrap();
    assert!(
        info.wit.contains("add: func(a: u32, b: u32) -> u32"),
        "{}",
        info.wit
    );
    let add = info
        .functions
        .iter()
        .find(|f| f.path == path("add"))
        .unwrap();
    assert_eq!(add.parameters[0].name, "a");
    assert!(add.callable);
    assert_eq!(
        runtime
            .call("math", &path("add"), &arguments(json!({"a": 20, "b": 22})))
            .unwrap(),
        json!({"results": [42]})
    );
    assert_eq!(
        runtime
            .call(
                "math",
                &path("second"),
                &arguments(json!({"pair": {"first": 1, "second": 9}}))
            )
            .unwrap(),
        json!({"results": [9]})
    );
}

#[test]
fn hot_connect_and_failed_replacement_are_atomic() {
    let runtime = Runtime::new().unwrap();
    runtime.connect("first", fixture()).unwrap();
    runtime.connect("second", fixture()).unwrap();
    assert_eq!(
        runtime.list().unwrap()["plugins"].as_array().unwrap().len(),
        2
    );
    assert!(runtime.connect("first", "/does/not/exist.wasm").is_err());
    assert_eq!(
        runtime
            .call("first", &path("add"), &arguments(json!({"a": 1, "b": 2})))
            .unwrap()["results"],
        json!([3])
    );
    let changed = include_str!("../fixtures/calculator.wat").replace("i32.add", "i32.mul");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("replacement.wasm");
    std::fs::write(&file, wat::parse_str(changed).unwrap()).unwrap();
    runtime.connect("first", &file).unwrap();
    assert_eq!(
        runtime
            .call("first", &path("add"), &arguments(json!({"a": 6, "b": 7})))
            .unwrap()["results"],
        json!([42])
    );
    assert_eq!(
        runtime
            .call("second", &path("add"), &arguments(json!({"a": 6, "b": 7})))
            .unwrap()["results"],
        json!([13])
    );
}

#[test]
fn bad_arguments_leave_instances_usable_and_traps_require_reconnection() {
    let runtime = Runtime::new().unwrap();
    runtime.connect("math", fixture()).unwrap();
    for bad in [
        json!({"a": -1, "b": 2}),
        json!({"a": 4294967296u64, "b": 2}),
        json!({"a": 1, "wrong": 2}),
        json!({"a": 1, "b": 2, "extra": 3}),
    ] {
        assert!(runtime.call("math", &path("add"), &arguments(bad)).is_err());
    }
    assert!(
        runtime
            .call("missing", &path("add"), &arguments(json!({})))
            .is_err()
    );
    assert!(
        runtime
            .call("math", &["missing".into()], &arguments(json!({})))
            .is_err()
    );
    assert_eq!(
        runtime
            .call("math", &path("add"), &arguments(json!({"a": 1, "b": 2})))
            .unwrap()["results"],
        json!([3])
    );
    for function in ["spin", "trap"] {
        runtime.connect("math", fixture()).unwrap();
        assert!(
            runtime
                .call("math", &[function.into()], &arguments(json!({})))
                .is_err()
        );
    }
    let error = runtime
        .call("math", &path("add"), &arguments(json!({"a": 1, "b": 2})))
        .unwrap_err();
    assert!(error.to_string().contains("is faulted"));
    runtime.connect("math", fixture()).unwrap();
    assert_eq!(
        runtime
            .call("math", &path("add"), &arguments(json!({"a": 1, "b": 2})))
            .unwrap()["results"],
        json!([3])
    );
}

#[test]
fn rejects_unbound_imports_without_publishing() {
    let runtime = Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("import.wat");
    std::fs::write(&file, "(component (import \"host\" (func)))").unwrap();
    let error = runtime
        .connect("needs-host", &file)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unresolved imports: host"), "{error}");
    assert_eq!(runtime.list().unwrap()["plugins"], json!([]));
}

#[test]
fn transfers_strings_and_lists_through_guest_memory() {
    let runtime = Runtime::new().unwrap();
    runtime
        .connect(
            "echo",
            format!("{}/fixtures/echo.wat", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    for text in ["", "Hello, 世界 🦀"] {
        assert_eq!(
            runtime
                .call("echo", &["echo".into()], &arguments(json!({"text": text})))
                .unwrap()["results"],
            json!([text])
        );
    }
    for bytes in [json!([]), json!([0, 1, 128, 255])] {
        assert_eq!(
            runtime
                .call(
                    "echo",
                    &["echo-bytes".into()],
                    &arguments(json!({"bytes": bytes}))
                )
                .unwrap()["results"],
            json!([bytes])
        );
    }
}

#[test]
fn enforces_memory_limits_during_instantiation() {
    let runtime = Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large.wat");
    std::fs::write(
        &file,
        "(component (core module $m (memory 1025)) (core instance (instantiate $m)))",
    )
    .unwrap();
    let error = runtime.connect("large", file).unwrap_err();
    assert!(format!("{error:#}").contains("memory"), "{error:#}");
    assert_eq!(runtime.list().unwrap()["plugins"], json!([]));
}

#[test]
fn successful_calls_refill_fuel_while_preserving_guest_globals() {
    let runtime = Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("work.wat");
    std::fs::write(&path, r#"(component
      (core module $m
        (global $count (mut i32) (i32.const 0))
        (func (export "work") (param $iterations i32) (result i32)
          block $done loop $next
            local.get $iterations i32.eqz br_if $done
            local.get $iterations i32.const 1 i32.sub local.set $iterations br $next
          end end
          global.get $count i32.const 1 i32.add global.set $count global.get $count))
      (core instance $i (instantiate $m))
      (func (export "work") (param "iterations" u32) (result u32) (canon lift (core func $i "work"))))"#).unwrap();
    runtime.connect("work", path).unwrap();
    for count in 1..=6 {
        assert_eq!(
            runtime
                .call(
                    "work",
                    &["work".into()],
                    &arguments(json!({"iterations": 500_000}))
                )
                .unwrap()["results"],
            json!([count])
        );
    }
}

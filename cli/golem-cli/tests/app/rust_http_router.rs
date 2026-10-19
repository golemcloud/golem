// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use test_r::{tag, test, timeout};

#[test]
#[tag(agents_guest_bridge)]
#[timeout("15 minutes")]
async fn rust_http_router_compile_diagnostics() {
    tokio::task::spawn_blocking(|| {
    let root = tempfile::tempdir().unwrap();
    let sdk = crate::workspace_path().join("sdks/rust/golem-rust");
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"), format!(r#"
[package]
name = "router-diagnostics"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
golem-rust = {{ path = {sdk:?}, features = ["export_golem_agentic"] }}
"#)).unwrap();
    let check = |source: &str| {
        std::fs::write(root.path().join("src/lib.rs"), source).unwrap();
        std::process::Command::new("cargo")
            .args(["check", "--quiet"])
            .env("CARGO_TARGET_DIR", crate::workspace_path().join("sdks/rust/target"))
            .current_dir(root.path())
            .output().expect("cargo check")
    };
    let source = |options: &str, methods: &str| format!(r#"
use golem_rust::http_router;
use golem_rust::agentic::{{Config, HttpRequest, HttpResponse, HttpRouter}};
struct Site;
#[http_router({options})]
impl HttpRouter for Site {{
    type Config = ();
    fn new(_: Config<()>) -> Self {{ Self }}
    {methods}
}}
"#);
    let options = r#"name = "Site", mount = "/""#;
    let output = check(&source(options, ""));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    for name in ["RouterAgent", "__GolemHttpRouterAgent0", "____GolemHttpRouterAgent0Initiator"] {
        let output = check(&source(options, "").replace("Site", name));
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
    }
    for (id, options, methods, diagnostic) in [
        ("metadata-dynamic-router-mount", r#"name = "Site", mount = "/{id}""#, "", "router mount must be a literal"),
        ("mapping-duplicate-decoded-source", r#"name = "Site", mount = "/", static_files = [("/a", "/x"), ("/%61", "/x")]"#, "", "duplicate compiled source + target"),
        ("metadata-extra-method", options, "fn extra(&self) {}", "put helpers in an inherent impl"),
        ("metadata-buffered-handler", options, "async fn handle(&self, request: String) -> HttpResponse { panic!() }", "incompatible type for trait"),
        ("metadata-streaming-provider", options, "async fn openapi(&self) -> golem_rust::agentic::AgentStream<String> { panic!() }", "String"),
        ("metadata-endpoint-policy-rejected", options, "#[endpoint(any = \"/\", auth = true)] async fn handle(&self, request: HttpRequest) -> HttpResponse { panic!() }", "put HTTP policy on #[http_router]"),
        ("router-snapshot-option", r#"name = "Site", mount = "/", snapshotting = "enabled""#, "", "routers are always ephemeral"),
    ] {
        let output = check(&source(options, methods));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{id}: invalid declaration compiled");
        assert!(error.contains(diagnostic), "{id}: expected {diagnostic:?}, got {error}");
    }
    let output = check(&format!("{}\nfn client() {{ let _ = std::mem::size_of::<SiteClient>(); }}", source(options, "")));
    assert!(!output.status.success(), "tooling-router-clients");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot find type `SiteClient`"));
    }).await.unwrap();
}

// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{Output, TestContext, cmd, flag};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use tokio::sync::oneshot;

const MARKER: &str = "ox-marker-5531";
const APP_NAME: &str = "ox-export-app";
const PROBE: &str = "ExportProbe(\"ox-marker-5531-ctor\")";
const PANIC_PROBE: &str = "ExportProbe(\"panic-probe\")";
const LARGE_PROBE: &str = "ExportProbe(\"large-probe\")";
const LONG_PROBE: &str = "ExportProbe(\"long-probe\")";
const RECREATED_PROBE: &str = "ExportProbe(\"recreated-probe\")";
const KEPT_NUMBER: &str = "7391204117";

const PROBE_SOURCE: &str = indoc! { r#"
    use golem_rust::agentic::Config;
    use golem_rust::http::{self, Request};
    use golem_rust::{agent_definition, agent_implementation, ConfigSchema};

    #[derive(ConfigSchema)]
    pub struct ProbeConfig {
        pub mark: String,
    }

    #[agent_definition]
    pub trait ExportProbe {
        fn new(name: String, #[agent_config] config: Config<ProbeConfig>) -> Self;
        fn echo(&self, text: String) -> String;
        async fn fetch(&self, port: u16, text: String) -> u16;
        async fn fetch_unreachable(&self, text: String) -> bool;
        fn log_line(&self, text: String);
        fn fail(&self, text: String) -> String;
        async fn call_helper(&self, text: String) -> u64;
        fn read_env(&self) -> u64;
        fn record(&self, n: u64, items: Vec<String>, bytes: Vec<u8>) -> u64;
        fn large(&self, size: u64) -> String;
        fn log_many(&self, n: u32);
    }

    struct ExportProbeImpl {
        name: String,
        #[allow(dead_code)]
        config: Config<ProbeConfig>,
    }

    #[agent_implementation]
    impl ExportProbe for ExportProbeImpl {
        fn new(name: String, #[agent_config] config: Config<ProbeConfig>) -> Self {
            Self { name, config }
        }

        fn echo(&self, text: String) -> String {
            format!("{}:{text}", self.name)
        }

        async fn fetch(&self, port: u16, text: String) -> u16 {
            let request = Request::post(format!("http://localhost:{port}/{text}?probe={text}"))
                .header("x-probe", text.as_str())
                .body(text.clone().into_bytes())
                .unwrap();
            let response = http::send(request).await.unwrap();
            response.status().as_u16()
        }

        async fn fetch_unreachable(&self, text: String) -> bool {
            let request = Request::get(format!("http://{text}.invalid/{text}?probe={text}"))
                .body(Vec::new())
                .unwrap();
            http::send(request).await.is_ok()
        }

        fn log_line(&self, text: String) {
            println!("{text}");
        }

        fn fail(&self, text: String) -> String {
            panic!("{text}")
        }

        async fn call_helper(&self, text: String) -> u64 {
            ExportHelperClient::get("helper".to_string()).measure(text).await
        }

        fn read_env(&self) -> u64 {
            std::env::var("OX_PROBE_SECRET").unwrap_or_default().len() as u64
        }

        fn record(&self, n: u64, items: Vec<String>, bytes: Vec<u8>) -> u64 {
            n + items.len() as u64 + bytes.len() as u64
        }

        fn large(&self, size: u64) -> String {
            "x".repeat(size as usize)
        }

        fn log_many(&self, n: u32) {
            for i in 0..n {
                println!("line {i}");
            }
        }
    }

    #[agent_definition]
    pub trait ExportHelper {
        fn new(name: String) -> Self;
        fn measure(&self, text: String) -> u64;
    }

    struct ExportHelperImpl;

    #[agent_implementation]
    impl ExportHelper for ExportHelperImpl {
        fn new(_name: String) -> Self {
            Self
        }

        fn measure(&self, text: String) -> u64 {
            text.len() as u64
        }
    }
"# };

/// Forwards every request unchanged to the router and records its method, path and query.
struct RecordingProxy {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    hold: Arc<Mutex<Option<Hold>>>,
}

/// Holds one request before it is forwarded: the first request that `matches` accepts after
/// `skip` accepted ones.
struct Hold {
    matches: fn(&str) -> bool,
    skip: usize,
    held: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl RecordingProxy {
    async fn start(target: String) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let hold = Arc::new(Mutex::new(None::<Hold>));
        let recorded = requests.clone();
        let holds = hold.clone();
        let client = reqwest::Client::new();
        let proxy = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = recorded.clone();
            let holds = holds.clone();
            let client = client.clone();
            let target = target.clone();
            async move {
                let (parts, body) = request.into_parts();
                let line = format!("{} {}", parts.method, parts.uri);
                let release = {
                    let mut holds = holds.lock().unwrap();
                    match holds.as_mut() {
                        Some(rule) if (rule.matches)(&line) && rule.skip > 0 => {
                            rule.skip -= 1;
                            None
                        }
                        Some(rule) if (rule.matches)(&line) => holds.take().map(|rule| {
                            let _ = rule.held.send(());
                            rule.release
                        }),
                        _ => None,
                    }
                };
                recorded.lock().unwrap().push(line);
                if let Some(release) = release {
                    let _ = release.await;
                }
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let mut headers = parts.headers.clone();
                headers.remove(reqwest::header::HOST);
                let response = client
                    .request(parts.method, format!("{target}{}", parts.uri))
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                let mut headers = response.headers().clone();
                for name in [
                    reqwest::header::CONTENT_LENGTH,
                    reqwest::header::CONTENT_ENCODING,
                    reqwest::header::TRANSFER_ENCODING,
                ] {
                    headers.remove(name);
                }
                let body = response.bytes().await.unwrap();
                let mut response = axum::response::Response::new(axum::body::Body::from(body));
                *response.status_mut() = status;
                *response.headers_mut() = headers;
                response
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, proxy).await.unwrap() });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            requests,
            hold,
        }
    }

    /// Holds the request that `matches` accepts after `skip` accepted ones. The first receiver
    /// fires when the request arrives; the request is forwarded when the sender fires or drops.
    fn hold(
        &self,
        matches: fn(&str) -> bool,
        skip: usize,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (held, on_held) = oneshot::channel();
        let (release, on_release) = oneshot::channel();
        *self.hold.lock().unwrap() = Some(Hold {
            matches,
            skip,
            held,
            release: on_release,
        });
        (on_held, release)
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

/// A local HTTP server that answers every request with status 207.
async fn start_multi_status_server() -> u16 {
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::MULTI_STATUS,
            format!("{MARKER} response"),
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// Deploys the probe app on a server that keeps oplog entries above 64 KiB out of oplog pages,
/// fills the probe agents, and points the CLI at a recording proxy in front of the router.
async fn setup_probe_app(ctx: &mut TestContext) -> RecordingProxy {
    let config_file = ctx.working_dir.join("golem-server.toml");
    std::fs::write(
        &config_file,
        indoc! { r#"
            [worker_executor.limits]
            max_inline_oplog_entry_size = 65536
        "# },
    )
    .unwrap();
    ctx.add_server_args(["--config", config_file.to_str().unwrap()]);
    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, APP_NAME, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());
    ctx.cd(APP_NAME);

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: {APP_NAME}

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              {APP_NAME}:rust-main:
                templates: rust
                env:
                  OX_PROBE_SECRET: {MARKER}-env

            agents:
              ExportProbe:
                config:
                  mark: ''
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("src/counter_agent.rs"), PROBE_SOURCE).unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let config = format!("mark=\"{MARKER}-config\"");
    let outputs = ctx
        .cli([flag::YES, cmd::AGENT, cmd::NEW, PROBE, "--config", &config])
        .await;
    assert!(outputs.success_or_dump());

    let port = start_multi_status_server().await.to_string();
    let marker_text = format!("\"{MARKER}\"");
    let record_args = [KEPT_NUMBER, "[\"a\", \"b\", \"c\"]", "[1, 2, 3, 4, 5]"];
    let mut calls: Vec<(&str, Vec<&str>)> = vec![
        (PROBE, vec!["echo", &marker_text]),
        (PROBE, vec!["fetch", &port, &marker_text]),
        (PROBE, vec!["fetch_unreachable", &marker_text]),
        (PROBE, vec!["log_line", &marker_text]),
        (PROBE, vec!["call_helper", &marker_text]),
        (PROBE, vec!["read_env"]),
    ];
    let mut record = vec!["record"];
    record.extend(record_args);
    calls.push((PROBE, record.clone()));
    calls.push((PROBE, vec!["echo", "\"between the records\""]));
    calls.push((PROBE, record));
    calls.push((LARGE_PROBE, vec!["large", "200000"]));
    calls.push((LARGE_PROBE, vec!["large", "600000"]));
    calls.push((LONG_PROBE, vec!["log_many", "200"]));
    for (agent, method_and_args) in calls {
        let mut args = vec![flag::YES, cmd::AGENT, cmd::INVOKE, agent];
        args.extend(method_and_args);
        let outputs = ctx.cli(args).await;
        assert!(outputs.success_or_dump());
    }
    let panic_text = format!("\"{MARKER} panic\"");
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            PANIC_PROBE,
            "fail",
            &panic_text,
        ])
        .await;
    assert!(!outputs.success(), "the panicking invocation must fail");

    let router = ctx.env_mut()["GOLEM_BUILTIN_LOCAL_URL"].clone();
    let proxy = RecordingProxy::start(router).await;
    ctx.add_env_var("GOLEM_BUILTIN_LOCAL_URL", proxy.url.clone());
    proxy
}

/// Texts that must not appear in an obfuscated report of `agent`: the marker as text, as base64
/// (standard and URL-safe, at the three byte alignments) and as its decimal byte sequence, and
/// the identity of the agent.
async fn forbidden_texts(ctx: &TestContext, agent: &str) -> Vec<String> {
    let mut forbidden = vec![
        MARKER.to_string(),
        agent.to_string(),
        serde_json::to_string(agent)
            .unwrap()
            .trim_matches('"')
            .to_string(),
        format!("{APP_NAME}:rust-main"),
    ];
    for offset in 0..3usize {
        let mut bytes = vec![0u8; offset];
        bytes.extend(MARKER.as_bytes());
        let start = (offset * 8).div_ceil(6);
        let end = bytes.len() * 8 / 6;
        for engine in [STANDARD_NO_PAD, URL_SAFE_NO_PAD] {
            forbidden.push(engine.encode(&bytes)[start..end].to_string());
        }
    }
    let decimal = MARKER.bytes().map(|b| b.to_string()).collect::<Vec<_>>();
    forbidden.push(decimal.join(","));
    forbidden.push(decimal.join(", "));

    let mut ids = Vec::new();
    for args in [
        [cmd::AGENT, cmd::GET, agent, flag::FORMAT, "json"].as_slice(),
        [cmd::COMPONENT, cmd::GET, flag::FORMAT, "json"].as_slice(),
    ] {
        let outputs = ctx.cli(args).await;
        assert!(outputs.success_or_dump());
        for document in outputs.stdout_json::<Value>() {
            collect_ids(&document, &mut ids);
        }
    }
    for key in ["componentId", "environmentId"] {
        assert!(
            ids.iter().any(|(found, _)| found == key),
            "no {key} in the agent and component metadata"
        );
    }
    forbidden.extend(ids.into_iter().map(|(_, id)| id));
    forbidden
}

/// The component and environment IDs in `value`, with their key.
fn collect_ids(value: &Value, ids: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value) {
                    ("componentId" | "environmentId", Value::String(id)) => {
                        ids.push((key.clone(), id.clone()))
                    }
                    _ => collect_ids(value, ids),
                }
            }
        }
        Value::Array(values) => values.iter().for_each(|value| collect_ids(value, ids)),
        _ => {}
    }
}

fn assert_no_leak(text: &str, forbidden: &[String], context: &str) {
    for needle in forbidden {
        assert!(
            !text.contains(needle.as_str()),
            "{context}: the report contains {needle:?}"
        );
    }
}

fn documents_of_type<'a>(documents: &'a [Value], document_type: &str) -> Vec<&'a Value> {
    documents
        .iter()
        .filter(|document| document["$type"] == document_type)
        .collect()
}

/// Every JSON object in `value`, depth first.
fn objects(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    let mut found = Vec::new();
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                found.push(map);
                stack.extend(map.values());
            }
            Value::Array(values) => stack.extend(values),
            _ => {}
        }
    }
    found
}

/// The value of the first report field named `name` in `node`.
fn report_field<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    objects(node)
        .into_iter()
        .find(|object| object.get("name") == Some(&Value::String(name.to_string())))
        .and_then(|object| object.get("value"))
}

/// Replaces every keyed token (`<domain>:<32 hex chars>`) with its domain.
fn without_tokens(value: &Value) -> Value {
    match value {
        Value::String(text) => match text.rsplit_once(':') {
            Some((domain, hex))
                if hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                Value::String(format!("{domain}:*"))
            }
            _ => value.clone(),
        },
        Value::Array(values) => Value::Array(values.iter().map(without_tokens).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), without_tokens(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

async fn plain_dump(ctx: &TestContext, agent: &str) -> Vec<Value> {
    let outputs = ctx
        .cli([cmd::AGENT, "oplog", agent, flag::FORMAT, "json"])
        .await;
    assert!(outputs.success_or_dump());
    outputs.stdout_json::<Value>()
}

async fn export(ctx: &TestContext, agent: &str, extra: &[&str]) -> Output {
    let mut args = vec![cmd::AGENT, "oplog", agent, "--obfuscate"];
    args.extend(extra);
    let outputs = ctx.cli(args).await;
    assert!(outputs.success_or_dump());
    outputs
}

async fn export_json(ctx: &TestContext, agent: &str, extra: &[&str]) -> Vec<Value> {
    let mut args = extra.to_vec();
    args.extend([flag::FORMAT, "json"]);
    export(ctx, agent, &args).await.stdout_json::<Value>()
}

fn entry_indices(documents: &[Value], document_type: &str) -> Vec<u64> {
    documents_of_type(documents, document_type)
        .into_iter()
        .map(|document| document["index"].as_u64().unwrap())
        .collect()
}

/// The indices of the plain dump entries that started an invocation of `method`.
fn invocation_indices(plain: &[Value], method: &str) -> Vec<u64> {
    plain
        .iter()
        .filter(|document| {
            document["entry"]["type"] == "AgentInvocationStarted"
                && document["entry"]["invocation"]["methodName"] == method
        })
        .map(|document| document["index"].as_u64().unwrap())
        .collect()
}

fn entry_at(documents: &[Value], index: u64) -> &Value {
    documents
        .iter()
        .find(|document| {
            document["$type"] == "agent.oplog-export-entry" && document["index"] == index
        })
        .unwrap_or_else(|| panic!("no export entry at index {index}"))
}

/// The value of the query parameter `name` of a recorded request.
fn query_param<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn served_url(serving_line: &str) -> String {
    serving_line[serving_line.find("http://").unwrap()..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

#[test]
#[timeout("40 minutes")]
async fn oplog_export_hides_customer_data() {
    let mut ctx = TestContext::new();
    let proxy = setup_probe_app(&mut ctx).await;

    // Control: the marker is in the oplog, also in the agent config and in the URL of the failed
    // HTTP call, and the query matches entries.
    let plain = plain_dump(&ctx, PROBE).await;
    let plain_text = serde_json::to_string(&plain).unwrap();
    for marked in [
        format!("{MARKER}-ctor"),
        format!("{MARKER}-config"),
        format!("{MARKER}-env"),
        format!("{MARKER}.invalid"),
    ] {
        assert!(
            plain_text.contains(&marked),
            "{marked} is not in {plain_text}"
        );
    }
    let queried = export_json(&ctx, PROBE, &["--query", MARKER]).await;
    assert!(!documents_of_type(&queried, "agent.oplog-export-entry").is_empty());
    assert_eq!(
        documents_of_type(&queried, "agent.oplog-export-header")[0]["filtered"],
        true
    );

    // No leak, in every format and with and without a query or kept scalars.
    for agent in [PROBE, PANIC_PROBE] {
        let forbidden = forbidden_texts(&ctx, agent).await;
        for selection in [&[][..], &["--query", MARKER][..]] {
            for keep in [&[][..], &["--keep-scalars"][..]] {
                let mut flags = selection.to_vec();
                flags.extend(keep);
                for format in ["json", "yaml", "text"] {
                    let mut args = flags.clone();
                    args.extend([flag::FORMAT, format]);
                    let outputs = export(&ctx, agent, &args).await;
                    assert_no_leak(
                        &outputs.stdout_text(),
                        &forbidden,
                        &format!("{agent} {args:?}"),
                    );
                }
                let html_path = ctx.cwd_path_join("report/export.html");
                let mut args = flags.clone();
                args.extend(["--html", html_path.to_str().unwrap()]);
                let outputs = export(&ctx, agent, &args).await;
                assert_no_leak(&outputs.stdout_text(), &forbidden, &format!("{args:?}"));
                assert_no_leak(
                    &fs::read_to_string(&html_path).unwrap(),
                    &forbidden,
                    &format!("{agent} {args:?} page"),
                );
            }
        }
    }

    // Document order.
    let documents = export_json(&ctx, PROBE, &[]).await;
    assert_eq!(documents[0]["$type"], "agent.oplog-export-header");
    assert_eq!(
        documents.last().unwrap()["$type"],
        "agent.oplog-export-coverage"
    );
    let indices = entry_indices(&documents, "agent.oplog-export-entry");
    assert_eq!(indices.len(), documents.len() - 2);
    assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));

    // --keep-scalars and --keep-shape at the arguments of `record`.
    let record_index = invocation_indices(&plain, "record")[0];
    let default_record = entry_at(&documents, record_index);
    assert!(
        !serde_json::to_string(&documents)
            .unwrap()
            .contains(KEPT_NUMBER)
    );
    assert!(
        objects(default_record)
            .iter()
            .all(|object| !object.contains_key("length")),
        "{default_record}"
    );
    let kept_scalars = export_json(&ctx, PROBE, &["--keep-scalars"]).await;
    assert!(
        objects(entry_at(&kept_scalars, record_index))
            .iter()
            .any(|object| object.get("kind") == Some(&Value::from("scalar"))
                && object.get("value") == Some(&Value::from(7391204117u64))),
        "{}",
        entry_at(&kept_scalars, record_index)
    );
    let kept_shape = export_json(&ctx, PROBE, &["--keep-shape"]).await;
    let shaped_record = objects(entry_at(&kept_shape, record_index));
    assert!(
        shaped_record
            .iter()
            .any(|object| object.get("length") == Some(&Value::from(3))),
        "{}",
        entry_at(&kept_shape, record_index)
    );
    let bytes_node = shaped_record
        .iter()
        .find(|object| object.get("length") == Some(&Value::from(5)))
        .unwrap_or_else(|| panic!("{}", entry_at(&kept_shape, record_index)));
    assert!(!bytes_node.contains_key("elements"), "{bytes_node:?}");

    // --keep-error-text. The guest panic writes its message to stderr `Log` entries, which stay
    // tokens; the `Error` entry holds the trap text, a token by default and in clear with the flag.
    let error_text = |documents: &[Value]| {
        let errors = documents_of_type(documents, "agent.oplog-export-entry")
            .into_iter()
            .filter(|document| document["kind"] == "Error")
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{documents:?}");
        report_field(errors[0], "error").unwrap().clone()
    };
    let panic_default = export_json(&ctx, PANIC_PROBE, &[]).await;
    let default_error = error_text(&panic_default);
    assert_eq!(default_error["kind"], "str", "{default_error}");
    assert!(default_error.get("value").is_none(), "{default_error}");
    assert!(
        !serde_json::to_string(&panic_default)
            .unwrap()
            .contains("wasm trap")
    );
    let panic_kept = export_json(&ctx, PANIC_PROBE, &["--keep-error-text"]).await;
    let kept_error = error_text(&panic_kept);
    assert!(
        kept_error["value"]
            .as_str()
            .is_some_and(|text| text.contains("wasm trap")),
        "{kept_error}"
    );
    assert!(!serde_json::to_string(&panic_kept).unwrap().contains(MARKER));

    // Hydration within and above --max-entry-bytes.
    let entry_reads = |requests: &[String]| {
        requests
            .iter()
            .filter_map(|request| {
                let path = request.split_whitespace().nth(1)?.split('?').next()?;
                let (prefix, index) = path.rsplit_once('/')?;
                prefix
                    .ends_with("/oplog")
                    .then(|| index.parse::<u64>().ok())?
            })
            .collect::<Vec<_>>()
    };
    proxy.take();
    let limited = export_json(&ctx, LARGE_PROBE, &["--max-entry-bytes", "400000"]).await;
    let limited_reads = entry_reads(&proxy.take());
    let omitted = documents_of_type(&limited, "agent.oplog-export-entry")
        .into_iter()
        .filter(|document| document.get("omitted").is_some())
        .collect::<Vec<_>>();
    assert_eq!(omitted.len(), 1, "{limited:?}");
    let omitted_index = omitted[0]["index"].as_u64().unwrap();
    assert_eq!(omitted[0]["omitted"]["reason"], "tooLarge");
    assert!(omitted[0]["omitted"]["size"].as_u64().unwrap() > 400000);
    let coverage = documents_of_type(&limited, "agent.oplog-export-coverage")[0];
    assert!(
        coverage["hydrationFailures"]["shown"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["index"] == omitted_index),
        "{coverage}"
    );
    assert_eq!(coverage["incomplete"], true);
    assert_eq!(limited_reads.len(), 1, "{limited_reads:?}");
    assert_ne!(limited_reads[0], omitted_index);
    assert!(entry_at(&limited, limited_reads[0]).get("body").is_some());

    let unlimited = export_json(&ctx, LARGE_PROBE, &[]).await;
    assert_eq!(entry_reads(&proxy.take()).len(), 2);
    assert!(
        documents_of_type(&unlimited, "agent.oplog-export-entry")
            .iter()
            .all(|document| document.get("body").is_some())
    );

    // An `End` read without its `Start` keeps the outcome of the HTTP call.
    let http_end = plain
        .iter()
        .filter(|document| document["entry"]["type"] == "End")
        .find(|document| {
            let start = document["entry"]["startIndex"].as_u64();
            plain.iter().any(|candidate| {
                candidate["entry"]["type"] == "Start"
                    && candidate["index"].as_u64() == start
                    && candidate["entry"]["functionName"] == "http::client::send"
            })
        })
        .unwrap_or_else(|| panic!("no End of an HTTP call in {plain:?}"));
    let end_index = http_end["index"].as_u64().unwrap();
    let start_index = http_end["entry"]["startIndex"].as_u64().unwrap();
    let from_end = export_json(&ctx, PROBE, &["--from", &end_index.to_string()]).await;
    let requests = proxy.take();
    let end_entry = entry_at(&from_end, end_index);
    assert!(
        objects(end_entry)
            .iter()
            .any(|object| object.values().any(|value| value == &Value::from(207))),
        "{end_entry}"
    );
    let start_index = start_index.to_string();
    let start_reads = requests
        .iter()
        .filter(|request| {
            query_param(request, "from") == Some(start_index.as_str())
                && query_param(request, "count") == Some("1")
        })
        .count();
    assert_eq!(start_reads, 1, "{requests:?}");

    // A long oplog is read page by page to its end.
    ctx.add_env_var("GOLEM_HTTP_BATCH_SIZE", "2");
    let long_plain = plain_dump(&ctx, LONG_PROBE).await;
    proxy.take();
    let long_export = export_json(&ctx, LONG_PROBE, &[]).await;
    let long_requests = proxy.take();
    ctx.env_mut().remove("GOLEM_HTTP_BATCH_SIZE");
    assert_eq!(
        entry_indices(&long_export, "agent.oplog-export-entry"),
        entry_indices(&long_plain, "agent.oplog"),
    );
    assert!(
        long_requests
            .iter()
            .filter(|request| query_param(request, "cursor").is_some())
            .count()
            > 100,
        "{}",
        long_requests.len()
    );

    // The plain command is unchanged.
    let plain_again = plain_dump(&ctx, PROBE).await;
    assert!(
        plain_again
            .iter()
            .all(|document| document["$type"] == "agent.oplog")
    );
    assert_eq!(
        entry_indices(&plain_again, "agent.oplog"),
        entry_indices(&plain, "agent.oplog")
    );
    let outputs = ctx
        .cli([
            cmd::AGENT,
            "oplog",
            LARGE_PROBE,
            "--omit-payloads",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_text().contains("Omitted"));
    assert!(
        proxy
            .take()
            .iter()
            .all(|request| query_param(request, "from") != Some("1")
                || query_param(request, "count") != Some("1")),
    );

    // --html with --format json writes the page and the documents.
    let html_path = ctx.cwd_path_join("report/structured.html");
    let documents = export_json(&ctx, PROBE, &["--html", html_path.to_str().unwrap()]).await;
    for document_type in [
        "agent.oplog-export-header",
        "agent.oplog-export-entry",
        "agent.oplog-export-coverage",
    ] {
        assert!(!documents_of_type(&documents, document_type).is_empty());
    }
    assert!(html_path.is_file());

    // A root that is deleted and created again during a one-shot export fails the export, and
    // the existing page stays unchanged. The proxy holds the second read of the `Create` entry,
    // the final root check, until the agent is recreated.
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            RECREATED_PROBE,
            "echo",
            "\"first\"",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let html_path = ctx.cwd_path_join("report/recreated.html");
    let html_arg = html_path.to_str().unwrap();
    export(&ctx, RECREATED_PROBE, &["--html", html_arg]).await;
    let page_before = fs::read_to_string(&html_path).unwrap();
    let report_files = || {
        let mut names = std::fs::read_dir(html_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let files_before = report_files();
    ctx.add_env_var("GOLEM_HTTP_BATCH_SIZE", "2");
    let is_create_read = |request: &str| {
        request
            .split_whitespace()
            .nth(1)
            .and_then(|uri| uri.split('?').next())
            .is_some_and(|path| path.ends_with("/oplog"))
            && query_param(request, "from") == Some("1")
            && query_param(request, "count") == Some("1")
    };
    let (held, release) = proxy.hold(is_create_read, 1);
    let failed_export = ctx.cli([
        cmd::AGENT,
        "oplog",
        RECREATED_PROBE,
        "--obfuscate",
        "--html",
        html_arg,
        flag::FORMAT,
        "json",
    ]);
    let recreate = async {
        tokio::time::timeout(std::time::Duration::from_secs(300), held)
            .await
            .expect("the export never read the Create entry a second time")
            .unwrap();
        let outputs = ctx
            .cli([flag::YES, cmd::AGENT, "delete", RECREATED_PROBE])
            .await;
        assert!(outputs.success_or_dump());
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                RECREATED_PROBE,
                "echo",
                "\"second\"",
            ])
            .await;
        assert!(outputs.success_or_dump());
        release.send(()).unwrap();
    };
    let (outputs, ()) = tokio::join!(failed_export, recreate);
    assert!(
        !outputs.success(),
        "the export of a recreated root must fail"
    );
    assert!(
        outputs
            .stderr_text()
            .contains("deleted and created again during the export"),
        "{}",
        outputs.stderr_text()
    );
    ctx.env_mut().remove("GOLEM_HTTP_BATCH_SIZE");
    assert!(
        outputs
            .stdout_json::<Value>()
            .iter()
            .all(|document| document["$type"] != "agent.oplog-export-coverage"),
        "{}",
        outputs.stdout_text()
    );
    assert_eq!(fs::read_to_string(&html_path).unwrap(), page_before);
    assert_eq!(report_files(), files_before);
}

#[test]
#[timeout("40 minutes")]
async fn oplog_export_serves_obfuscated_pages() {
    let mut ctx = TestContext::new();
    let proxy = setup_probe_app(&mut ctx).await;
    let plain = plain_dump(&ctx, PROBE).await;
    let record_indices = invocation_indices(&plain, "record");
    assert_eq!(record_indices.len(), 2);
    let forbidden = forbidden_texts(&ctx, PROBE).await;

    let serve = [
        cmd::AGENT,
        "oplog",
        PROBE,
        "--obfuscate",
        "--serve",
        "--port",
        "0",
        "--no-open",
    ];
    let client = reqwest::Client::new();
    let get = async |url: &str| client.get(url).send().await.unwrap();

    // The `functionInput` of each `record` call, read page by page.
    let record_inputs = async |url: &str| {
        let mut inputs = Vec::new();
        let mut query = String::new();
        loop {
            let page: Value = get(&format!("{url}api/oplog?count=2{query}"))
                .await
                .json()
                .await
                .unwrap();
            assert_no_leak(&page.to_string(), &forbidden, "served page");
            for entry in page["entries"].as_array().unwrap() {
                if record_indices.contains(&entry["index"].as_u64().unwrap()) {
                    inputs.push(report_field(entry, "functionInput").unwrap().clone());
                }
            }
            let next = &page["next"];
            if !next.is_object() {
                break;
            }
            query = format!(
                "&cursorIndex={}&cursorRevision={}",
                next["nextOplogIndex"], next["currentComponentRevision"]
            );
        }
        inputs
    };

    let (mut server, serving_line) = ctx
        .cli_process_after_line(serve, "Serving oplog viewer at")
        .await;
    let url = served_url(&serving_line);

    let page = get(&url).await.text().await.unwrap();
    assert_no_leak(&page, &forbidden, "served viewer");
    let document = super::agents::embedded_oplog_viewer_document(&page);
    assert_eq!(document["mode"], "liveexport");
    for field in ["agentId", "componentName", "componentId"] {
        assert!(document.get(field).is_none(), "{document}");
    }

    let first: Value = get(&format!("{url}api/oplog?count=2"))
        .await
        .json()
        .await
        .unwrap();
    let again: Value = get(&format!("{url}api/oplog?count=2"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(first, again);

    let inputs = record_inputs(&url).await;
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], inputs[1]);

    let metadata = get(&format!("{url}api/agent")).await.text().await.unwrap();
    assert_no_leak(&metadata, &forbidden, "api/agent");
    let metadata: Value = serde_json::from_str(&metadata).unwrap();
    assert!(metadata.get("lastError").is_none(), "{metadata}");
    assert!(metadata.get("agentId").is_none(), "{metadata}");

    // Another process uses another key.
    let (mut second, second_line) = ctx
        .cli_process_after_line(serve, "Serving oplog viewer at")
        .await;
    let second_inputs = record_inputs(&served_url(&second_line)).await;
    assert_ne!(second_inputs[0], inputs[0]);
    assert_eq!(
        without_tokens(&second_inputs[0]),
        without_tokens(&inputs[0])
    );
    second.start_kill().unwrap();
    second.wait().await.unwrap();
    let one_shot = export_json(&ctx, PROBE, &[]).await;
    let one_shot_input =
        report_field(entry_at(&one_shot, record_indices[0]), "functionInput").unwrap();
    assert_ne!(one_shot_input, &inputs[0]);

    // A recreated agent is not mixed into the pages of the first one. The proxy holds the main
    // read of the page, after the root check before it, until the agent is recreated, so only the
    // root check after the read can see the change.
    let (held, release) = proxy.hold(
        |request| {
            request
                .split_whitespace()
                .nth(1)
                .and_then(|uri| uri.split('?').next())
                .is_some_and(|path| path.ends_with("/oplog"))
                && query_param(request, "count") == Some("2")
                && query_param(request, "from").is_none()
        },
        0,
    );
    let page_url = format!("{url}api/oplog?count=2");
    let page_during_recreation = get(&page_url);
    let recreate = async {
        tokio::time::timeout(std::time::Duration::from_secs(300), held)
            .await
            .expect("the served viewer never read the page")
            .unwrap();
        let outputs = ctx.cli([flag::YES, cmd::AGENT, "delete", PROBE]).await;
        assert!(outputs.success_or_dump());
        let outputs = ctx
            .cli([flag::YES, cmd::AGENT, cmd::INVOKE, PROBE, "read_env"])
            .await;
        assert!(outputs.success_or_dump());
        release.send(()).unwrap();
    };
    let (response, ()) = tokio::join!(page_during_recreation, recreate);
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    let response = get(&format!("{url}api/agent")).await;
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);

    // A failed read answers a fixed text: the read of a deleted agent quotes its ID upstream.
    let outputs = ctx.cli([flag::YES, cmd::AGENT, "delete", PROBE]).await;
    assert!(outputs.success_or_dump());
    for endpoint in ["api/oplog?count=2", "api/agent"] {
        let response = get(&format!("{url}{endpoint}")).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
        assert_no_leak(&response.text().await.unwrap(), &forbidden, endpoint);
    }
    let outputs = ctx
        .cli([
            cmd::AGENT,
            "oplog",
            PROBE,
            "--obfuscate",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(
        !outputs.success(),
        "the export of a deleted agent must fail"
    );
    assert_no_leak(&outputs.stdout_text(), &forbidden, "failed export stdout");
    assert_no_leak(&outputs.stderr_text(), &forbidden, "failed export stderr");
    assert!(
        outputs.stderr_text().contains("A read of the agent failed"),
        "{}",
        outputs.stderr_text()
    );

    server.start_kill().unwrap();
    server.wait().await.unwrap();
}

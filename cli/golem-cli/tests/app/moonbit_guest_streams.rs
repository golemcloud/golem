use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use std::process::Stdio;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

inherit_test_dep!(Tracing);

async fn moonbit_guest_streams_context() -> TestContext {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("moonbit-stream-bridge")).unwrap();
    ctx.cd("moonbit-stream-bridge");
    for (template, component) in [("rust", "provider"), ("moonbit", "consumer")] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("moonbit-stream-bridge:{component}"),
            ])
            .await;
        assert!(outputs.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: moonbit-stream-bridge
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          moonbit-stream-bridge:provider:
            dir: provider
            templates: rust
          moonbit-stream-bridge:consumer:
            dir: consumer
            templates: moonbit
            dependencies:
              agents:
                - moonbit-stream-bridge:provider/StreamProvider
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("provider/src/counter_agent.rs"), indoc! {r#"
        use golem_rust::{agent_definition, agent_implementation, IntoSchema, FromSchema};
        use golem_rust::agentic::{AgentStream, spawn_local};

        #[derive(IntoSchema, FromSchema)]
        pub struct StreamItem { pub label: String, pub children: Vec<StreamItem> }

        #[derive(IntoSchema, FromSchema)]
        pub struct StreamBundle {
            pub optional: Option<AgentStream<StreamItem>>,
            pub siblings: Vec<AgentStream<StreamItem>>,
        }

        #[agent_definition]
        pub trait StreamProvider {
            fn new(name: String) -> Self;
            async fn consume(&self, input: AgentStream<i8>) -> i32;
            fn produce(&self) -> AgentStream<StreamItem>;
            fn forward(&self, bundle: StreamBundle) -> StreamBundle;
            fn nested(&self, input: AgentStream<AgentStream<StreamItem>>) -> AgentStream<AgentStream<StreamItem>>;
            async fn malformed(&self) -> AgentStream<u32>;
            async fn drop_after_one(&self, input: AgentStream<u32>) -> u32;
            fn status(&self) -> String;
        }
        struct StreamProviderImpl { name: String }
        #[agent_implementation]
        impl StreamProvider for StreamProviderImpl {
            fn new(name: String) -> Self { Self { name } }
            async fn consume(&self, mut input: AgentStream<i8>) -> i32 {
                let mut total = 0;
                while let Some(value) = input.next().await.expect("read narrow input") { total += i32::from(value); }
                total
            }
            fn produce(&self) -> AgentStream<StreamItem> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move {
                    let _ = writer.write_one(StreamItem { label: "remote".into(), children: vec![StreamItem { label: "child".into(), children: vec![] }] }).await;
                });
                stream
            }
            fn forward(&self, bundle: StreamBundle) -> StreamBundle { bundle }
            fn nested(&self, input: AgentStream<AgentStream<StreamItem>>) -> AgentStream<AgentStream<StreamItem>> { input }
            async fn malformed(&self) -> AgentStream<u32> {
                let gate = golem_rust::create_promise();
                StreamGateClient::get(self.name.clone()).arm(gate.clone()).await;
                let (mut writer, stream) = golem_rust::schema::wit::new_schema_value_stream();
                spawn_local(async move {
                    let first = golem_rust::schema::wit::encode_value(&golem_rust::schema::SchemaValue::U32(7)).expect("encode first item");
                    if writer.write_one(first).await.is_some() { return; }
                    golem_rust::get_promise(&gate).get().await;
                    let _ = writer.write_one(golem_rust::schema::wit::wire::SchemaValueTree {
                        value_nodes: vec![], root: 0,
                    }).await;
                });
                AgentStream::from_raw(stream)
            }
            async fn drop_after_one(&self, mut input: AgentStream<u32>) -> u32 {
                let first = input.next().await.expect("read input").expect("first item");
                drop(input);
                first
            }
            fn status(&self) -> String { "ready".into() }
        }

        #[agent_definition]
        pub trait StreamGate {
            fn new(name: String) -> Self;
            fn arm(&mut self, promise: golem_rust::PromiseId);
            fn release(&mut self) -> bool;
        }
        struct StreamGateImpl { promise: Option<golem_rust::PromiseId> }
        #[agent_implementation]
        impl StreamGate for StreamGateImpl {
            fn new(_name: String) -> Self { Self { promise: None } }
            fn arm(&mut self, promise: golem_rust::PromiseId) { self.promise = Some(promise); }
            fn release(&mut self) -> bool {
                golem_rust::complete_promise(&self.promise.take().expect("armed gate"), &[])
            }
        }
    "#}).unwrap();
    let manifest = ctx.cwd_path_join("moon.mod.json");
    let mut module: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    module["deps"]["stream-provider-guest-client"] = serde_json::json!({"path": "golem-temp/bridge-sdk/moonbit/internal/stream-provider-guest-client"});
    fs::write_str(
        &manifest,
        serde_json::to_string_pretty(&module).unwrap() + "\n",
    )
    .unwrap();
    let manifest = ctx.cwd_path_join("consumer/moon.pkg");
    let package = fs::read_to_string(&manifest).unwrap().replace(
        "import {",
        "import {\n  \"stream-provider-guest-client/client\" @provider,",
    );
    fs::write_str(&manifest, package).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/counter.mbt"),
        indoc! {r#"
        #derive.agent
        struct StreamConsumer { name : String }

        fn StreamConsumer::new(name : String) -> StreamConsumer { { name, } }

        pub async fn StreamConsumer::malformed(self : Self) -> @schema.AgentStream[UInt] {
          @provider.StreamProviderClient::scoped(self.name, async fn(remote) {
            let input = remote.malformed()
            @schema.AgentStream::produce(async fn(writer) {
              defer input.drop()
              for ;; {
                match input.read() {
                  Some(value) => if writer.write_one(value) is @schema.PeerDropped { return }
                  None => return
                }
              }
            }, on_unstarted_drop=() => input.drop())
          })
        }

        pub async fn StreamConsumer::peer_drop(self : Self) -> String {
          @provider.StreamProviderClient::scoped(self.name, async fn(remote) {
            let peer_dropped = Ref(false)
            let accepted = Ref(0)
            let input = @provider.produce_drop_after_one_input_0_stream(async fn(writer) {
              for ;; {
                match writer.write_one(23U) {
                  @schema.Accepted => accepted.val += 1
                  @schema.PeerDropped => { peer_dropped.val = true; return }
                }
              }
            })
            assert_eq(remote.drop_after_one(input), 23U)
            // RPC round trips yield to transport cancellation without sleeps.
            for _ in 0..<64 {
              assert_eq(remote.status(), "ready")
              if peer_dropped.val {
                assert_true(accepted.val >= 1)
                return "ok:peer-dropped"
              }
            }
            fail("MoonBit producer did not observe PeerDropped after remote reader drop")
          })
        }

        pub async fn StreamConsumer::run(self : Self) -> String {
          @provider.StreamProviderClient::scoped(self.name, async fn(remote) {
            let source = @provider.produce_consume_input_0_stream(async fn(writer) {
              assert_true(writer.write_all([1, 2, 3]) is @schema.Accepted)
            })
            assert_eq(remote.consume(source), 6)
            let output = remote.produce()
            let forwarded = remote.forward({ optional: Some(output), siblings: [] })
            let output = forwarded.optional.unwrap()
            let item = output.read().unwrap()
            assert_eq(item.label, "remote")
            assert_eq(item.children[0].label, "child")
            assert_true(output.read() is None)
            for count in 0..<4 {
              let siblings = []
              for _ in 0..<count { siblings.push(remote.produce()) }
              let returned = remote.forward({ optional: None, siblings, })
              assert_eq(returned.siblings.length(), count)
              for stream in returned.siblings { stream.drop() }
            }
            let outer = @provider.produce_nested_input_0_stream(async fn(writer) {
              let inner = @provider.produce_nested_input_0_0_stream(async fn(writer) {
                assert_true(writer.write_one({ label: "nested", children: [] }) is @schema.Accepted)
              })
              assert_true(writer.write_one(inner) is @schema.Accepted)
            })
            let outer = remote.nested(outer)
            let inner = outer.read().unwrap()
            assert_eq(inner.read().unwrap().label, "nested")
            assert_true(inner.read() is None)
            assert_true(outer.read() is None)
            "ok:" + remote.status()
          })
        }
        fn main {}
    "#},
    )
    .unwrap();
    assert!(ctx.cli([cmd::BUILD]).await.success_or_dump());
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    ctx
}

#[test]
#[timeout("20 minutes")]
async fn test_moonbit_generated_guest_streams_e2e() {
    let ctx = moonbit_guest_streams_context().await;
    let name = Uuid::new_v4();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("StreamConsumer(\"{name}\")"),
            "run",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("ok:ready"));
}

#[test]
#[timeout("20 minutes")]
async fn test_moonbit_generated_guest_streams_producer_failure() {
    let ctx = moonbit_guest_streams_context().await;
    let name = Uuid::new_v4();
    let mut child = Command::new(&ctx.golem_cli_path)
        .arg("--config-dir")
        .arg(ctx.config_dir.path())
        .args([
            cmd::AGENT,
            cmd::INVOKE,
            &format!("StreamConsumer(\"{name}\")"),
            "malformed",
            "--no-stream",
        ])
        .env_remove("GOLEM_BUILTIN_LOCAL_URL")
        .envs(&ctx.env)
        .current_dir(&ctx.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let stderr = tokio::spawn(async move {
        let mut text = String::new();
        stderr.read_to_string(&mut text).await.unwrap();
        text
    });
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let line = stdout
                .next_line()
                .await
                .unwrap()
                .expect("must deliver 7 before failure");
            if line == "7" {
                break;
            }
        }
        // An accepted relay write is not a CLI delivery acknowledgement.
        // Release the independent gate only after observing the exact item line.
        let release = ctx
            .cli([
                cmd::AGENT,
                cmd::INVOKE,
                &format!("StreamGate(\"{name}\")"),
                "release",
            ])
            .await;
        assert!(release.success_or_dump());
        assert!(release.stdout_contains("true"));
        while stdout.next_line().await.unwrap().is_some() {}
        let status = child.wait().await.unwrap();
        assert!(
            status.code().is_some_and(|code| code != 0),
            "malformed producer became clean EOF: {status}"
        );
    })
    .await
    .expect("malformed producer did not terminate the invocation within 90 seconds");
    let stderr = stderr.await.unwrap();
    assert!(
        stderr.contains("Output stream") || stderr.contains("Invocation Failed"),
        "expected stream/invocation failure, not an unrelated CLI error: {stderr}"
    );
}

#[test]
#[timeout("20 minutes")]
async fn test_moonbit_generated_guest_streams_peer_drop() {
    let ctx = moonbit_guest_streams_context().await;
    let name = Uuid::new_v4();
    let outputs = tokio::time::timeout(
        Duration::from_secs(90),
        ctx.cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("StreamConsumer(\"{name}\")"),
            "peer_drop",
        ]),
    )
    .await
    .expect("readable-drop acknowledgement did not complete within 90 seconds");
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("ok:peer-dropped"));
}

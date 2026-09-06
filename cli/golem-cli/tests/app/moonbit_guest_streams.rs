use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[test]
#[timeout("20 minutes")]
async fn test_moonbit_generated_guest_streams_e2e() {
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
            fn status(&self) -> String;
        }
        struct StreamProviderImpl;
        #[agent_implementation]
        impl StreamProvider for StreamProviderImpl {
            fn new(_name: String) -> Self { Self }
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
            fn status(&self) -> String { "ready".into() }
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

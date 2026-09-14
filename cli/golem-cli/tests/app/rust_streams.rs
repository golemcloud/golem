use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{tag, test, timeout};

#[test]
#[tag(agents_guest_bridge)]
#[timeout("15 minutes")]
async fn rust_generated_native_stream_bridge_e2e() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("native-streams")).unwrap();
    ctx.cd("native-streams");
    for component in ["native-streams:provider", "native-streams:consumer"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "rust",
                flag::COMPONENT_NAME,
                component,
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: native-streams
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          native-streams:provider:
            dir: provider
            templates: rust
          native-streams:consumer:
            dir: consumer
            templates: rust
            dependencies:
              agents:
                - native-streams:provider/StreamProvider
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("provider/src/counter_agent.rs"), indoc! {r#"
        use golem_rust::{agent_definition, agent_implementation, IntoSchema, FromSchema};
        use golem_rust::agentic::{AgentStream, spawn_local};
        use golem_rust::schema::{SchemaValue, SchemaType, SchemaBuilder, TypeId, FromSchemaError};

        pub struct FixedPair(Vec<u32>);
        impl IntoSchema for FixedPair {
            fn type_id() -> TypeId { TypeId::new("FixedPair") }
            fn register_in(_: &mut SchemaBuilder) -> SchemaType {
                SchemaType::fixed_list(SchemaType::u32(), 2)
            }
            fn to_value(&self) -> SchemaValue {
                SchemaValue::FixedList { elements: self.0.iter().map(|v| v.to_value()).collect() }
            }
        }
        impl FromSchema for FixedPair {
            fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
                match value {
                    SchemaValue::FixedList { elements } => Ok(Self(elements.iter().map(u32::from_value).collect::<Result<_, _>>()?)),
                    _ => panic!("expected fixed list"),
                }
            }
        }
        #[derive(IntoSchema, FromSchema)]
        pub struct FallibleItem {
            pub first: AgentStream<u32>,
            pub tail: FixedPair,
        }

        #[agent_definition]
        pub trait StreamProvider {
            fn new(name: String) -> Self;
            async fn consume(&self, input: AgentStream<u32>) -> Vec<u32>;
            fn produce(&self) -> AgentStream<u32>;
            fn exchange(&self, input: AgentStream<u32>) -> AgentStream<u32>;
            fn forward(&self, input: Vec<AgentStream<u32>>) -> Vec<AgentStream<u32>>;
            fn nested(&self, input: AgentStream<AgentStream<u32>>) -> AgentStream<AgentStream<u32>>;
            fn application_error(&self) -> AgentStream<Result<u32, String>>;
            fn malformed(&self) -> AgentStream<u32>;
            fn fixed_echo(&self, input: AgentStream<FixedPair>) -> AgentStream<FixedPair>;
            fn fallible_echo(&self, input: AgentStream<FallibleItem>) -> AgentStream<FallibleItem>;
            fn drop_input(&self, input: AgentStream<u32>);
            fn status(&self) -> u32;
        }
        struct Provider;
        #[agent_implementation]
        impl StreamProvider for Provider {
            fn new(_name: String) -> Self { Self }
            async fn consume(&self, input: AgentStream<u32>) -> Vec<u32> {
                input.collect().await.unwrap()
            }
            fn produce(&self) -> AgentStream<u32> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move { let _ = writer.write_all([1, 2, 3]).await; });
                stream
            }
            fn exchange(&self, mut input: AgentStream<u32>) -> AgentStream<u32> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move {
                    while let Some(item) = input.next().await.unwrap() {
                        if writer.write_one(item * 10).await.is_err() { break; }
                    }
                });
                stream
            }
            fn forward(&self, input: Vec<AgentStream<u32>>) -> Vec<AgentStream<u32>> { input }
            fn nested(&self, input: AgentStream<AgentStream<u32>>) -> AgentStream<AgentStream<u32>> { input }
            fn application_error(&self) -> AgentStream<Result<u32, String>> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move { writer.write_all([Ok(1), Err("recoverable".to_string()), Ok(2)]).await.unwrap(); });
                stream
            }
            fn malformed(&self) -> AgentStream<u32> {
                let (mut writer, stream) = golem_rust::schema::wit::new_schema_value_stream();
                spawn_local(async move {
                    let _ = writer.write_one(golem_rust::schema::wit::wire::SchemaValueTree { value_nodes: vec![], root: 0 }).await;
                });
                AgentStream::from_raw(stream)
            }
            fn fixed_echo(&self, input: AgentStream<FixedPair>) -> AgentStream<FixedPair> { input }
            fn fallible_echo(&self, input: AgentStream<FallibleItem>) -> AgentStream<FallibleItem> { input }
            fn drop_input(&self, input: AgentStream<u32>) { drop(input); }
            fn status(&self) -> u32 { 42 }
        }
    "#}).unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/counter_agent.rs"), indoc! {r#"
        use golem_rust::{agent_definition, agent_implementation};
        use golem_rust::agentic::{AgentStream, spawn_local};
        use stream_provider_guest_client::{StreamProvider, FallibleItem, new_u32_stream, new_fixed_list_stream, new_fallible_item_stream};

        #[agent_definition]
        pub trait StreamConsumer {
            fn new(name: String) -> Self;
            async fn run(&self, scenario: String) -> String;
        }
        struct Consumer;
        #[agent_implementation]
        impl StreamConsumer for Consumer {
            fn new(_name: String) -> Self { Self }
            async fn run(&self, scenario: String) -> String {
                let provider = StreamProvider::get(format!("target-{scenario}")).unwrap();
                let forwarder = StreamProvider::get(format!("forwarder-{scenario}")).unwrap();
                if scenario == "basics" {
                let (mut writer, input) = new_u32_stream();
                spawn_local(async move { writer.write_all([4, 5]).await.unwrap(); });
                assert_eq!(provider.consume(input).await.unwrap(), vec![4, 5]);
                assert_eq!(provider.produce().await.unwrap().collect().await.unwrap(), vec![1, 2, 3]);

                let (mut writer, input) = new_u32_stream();
                spawn_local(async move { writer.write_all([6, 7]).await.unwrap(); });
                assert_eq!(provider.exchange(input).await.unwrap().collect().await.unwrap(), vec![60, 70]);
                }

                if scenario == "forwarding" || scenario == "loopback" {
                let forwarder = if scenario == "loopback" { &provider } else { &forwarder };
                for count in [0, 1, 3] {
                    let mut inputs = Vec::new();
                    for _ in 0..count { inputs.push(provider.produce().await.unwrap()); }
                    let outputs = forwarder.forward(inputs).await.unwrap();
                    assert_eq!(outputs.len(), count);
                    for output in outputs { assert_eq!(output.collect().await.unwrap(), vec![1, 2, 3]); }
                }

                let (mut writer, input) = AgentStream::<AgentStream<u32>>::new();
                let inner = provider.produce().await.unwrap();
                spawn_local(async move { writer.write_one(inner).await.unwrap(); });
                let mut outer = forwarder.nested(input).await.unwrap();
                assert_eq!(outer.next().await.unwrap().unwrap().collect().await.unwrap(), vec![1, 2, 3]);
                assert!(outer.next().await.unwrap().is_none());
                }

                if scenario == "errors" {
                assert_eq!(provider.application_error().await.unwrap().collect().await.unwrap(), vec![Ok(1), Err("recoverable".to_string()), Ok(2)]);
                drop(provider.produce().await.unwrap());
                }

                if scenario == "peer_drop" {
                let (mut writer, input) = new_u32_stream();
                provider.drop_input(input).await.unwrap();
                while writer.write_one(99).await.is_ok() {}
                }

                if scenario == "fatal" {
                    if let Ok(mut malformed) = provider.malformed().await {
                        assert!(malformed.next().await.is_err());
                    }
                    return "native-streams-ok".to_string();
                }

                if scenario == "codecs" {
                // Codec-bound lifting and forwarding do not evaluate item codecs.
                let (mut writer, input) = new_u32_stream();
                let holder = input.into_schema_stream();
                let identity = holder.cell_id();
                let stream = AgentStream::<u32>::from_schema_stream(holder, |_| panic!("forwarding decoded an item"));
                let holder = stream.into_schema_stream();
                assert_eq!(identity, holder.cell_id());
                drop(holder);
                assert!(writer.write_one(1).await.is_err());

                let (mut writer, input) = new_u32_stream();
                let holder = input.into_schema_stream();
                let aliased = golem_rust::SchemaValue::Tuple { elements: vec![
                    golem_rust::SchemaValue::Stream(holder.clone()),
                    golem_rust::SchemaValue::Stream(holder.clone()),
                ] };
                assert!(golem_rust::encode_schema_value_async(&aliased).await.is_err());
                assert!(holder.is_present());
                drop(aliased);
                drop(holder);
                assert!(writer.write_one(1).await.is_err());

                let (mut writer, input) = new_fixed_list_stream();
                assert!(writer.write_one(vec![1]).await.is_err());
                spawn_local(async move { writer.write_one(vec![1, 2]).await.unwrap(); });
                assert_eq!(provider.fixed_echo(input).await.unwrap().collect().await.unwrap(), vec![vec![1, 2]]);

                // The generated item encoder acquires first before rejecting tail.
                let (mut nested_writer, nested) = new_u32_stream();
                let (mut writer, output) = new_fallible_item_stream();
                let error = writer.write_one(FallibleItem { first: nested, tail: vec![1] }).await.unwrap_err();
                assert!(error.contains("fixed-list"));
                assert!(nested_writer.write_one(1).await.is_err());
                drop(writer);
                drop(output);

                // A write remains pending until a reader accepts its item.
                let (mut writer, reader) = new_u32_stream();
                let mut write = Box::pin(async move { writer.write_one(7).await });
                let pending = std::future::poll_fn(|cx| {
                    std::task::Poll::Ready(std::future::Future::poll(write.as_mut(), cx).is_pending())
                }).await;
                assert!(pending);
                spawn_local(async move { write.await.unwrap(); });
                assert_eq!(provider.consume(reader).await.unwrap(), vec![7]);
                }

                assert_eq!(provider.status().await.unwrap(), 42);
                "native-streams-ok".to_string()
            }
        }
    "#}).unwrap();
    let manifest = ctx.cwd_path_join("consumer/Cargo.toml");
    let cargo = fs::read_to_string(&manifest).unwrap();
    fs::write_str(&manifest, cargo.replace("[dependencies]", indoc! {r#"
        [dependencies]
        stream-provider-guest-client = { path = "../golem-temp/bridge-sdk/rust/internal/stream-provider-guest-client" }
    "#}.trim_end())).unwrap();
    let output = ctx.cli([cmd::BUILD]).await;
    assert!(output.success_or_dump());
    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());
    let mut failed = Vec::new();
    for scenario in [
        "basics",
        "forwarding",
        "loopback",
        "errors",
        "fatal",
        "codecs",
        "peer_drop",
    ] {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            ctx.cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                &format!("StreamConsumer(\"{scenario}\")"),
                "run",
                &format!("\"{scenario}\""),
            ]),
        )
        .await;
        let Ok(output) = output else {
            failed.push(scenario);
            continue;
        };
        if scenario == "fatal"
            && !output.success()
            && output.stderr_contains("durable stream ended with error context")
        {
            continue;
        }
        if !output.success_or_dump() || !output.stdout_contains("native-streams-ok") {
            failed.push(scenario);
        }
    }
    assert!(
        failed.is_empty(),
        "native stream scenarios failed: {failed:?}"
    );
}

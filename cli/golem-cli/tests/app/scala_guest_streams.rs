// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{test, timeout};

async fn deployed_scala_streams_context() -> TestContext {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("scala-guest-streams")).unwrap();
    ctx.cd("scala-guest-streams");
    for (template, component) in [("rust", "provider"), ("scala", "consumer")] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("scala-guest-streams:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: scala-guest-streams
        environments:
          local:
            server: local
        components:
          scala-guest-streams:provider:
            dir: provider
            templates: rust
          scala-guest-streams:consumer:
            dir: consumer
            templates: scala
            dependencies:
              agents:
                - scala-guest-streams:provider/StreamProvider
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("provider/src/lib.rs"), indoc! {r#"
        use golem_rust::{agent_definition, agent_implementation, IntoSchema, FromSchema};
        use golem_rust::agentic::{AgentStream, spawn_local};

        #[derive(IntoSchema, FromSchema)]
        pub struct Item { pub label: String, pub children: Vec<Item> }
        #[derive(IntoSchema, FromSchema)]
        pub struct Bundle { pub optional: Option<AgentStream<Item>>, pub siblings: Vec<AgentStream<Item>> }

        #[agent_definition]
        pub trait StreamProvider {
            fn new(name: String) -> Self;
            fn exchange(&self, input: AgentStream<Item>) -> AgentStream<Item>;
            fn forward(&self, input: Bundle) -> Bundle;
            fn nested(&self, input: AgentStream<AgentStream<Item>>) -> AgentStream<AgentStream<Item>>;
            fn produce(&self) -> AgentStream<Item>;
            fn application_error(&self) -> AgentStream<Result<String, String>>;
            fn malformed(&self) -> AgentStream<u32>;
            async fn consume(&self, input: AgentStream<Item>) -> String;
            async fn hold(&self, input: AgentStream<Item>) -> String;
            fn drop_input(&self, input: AgentStream<Item>) -> String;
            fn status(&self) -> String;
        }
        struct StreamProviderImpl;
        #[agent_implementation]
        impl StreamProvider for StreamProviderImpl {
            fn new(_name: String) -> Self { Self }
            fn exchange(&self, input: AgentStream<Item>) -> AgentStream<Item> { input }
            fn forward(&self, input: Bundle) -> Bundle { input }
            fn nested(&self, input: AgentStream<AgentStream<Item>>) -> AgentStream<AgentStream<Item>> { input }
            fn produce(&self) -> AgentStream<Item> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move {
                    let _ = writer.write_all(vec![Item { label: "remote".into(), children: vec![] }]).await;
                });
                stream
            }
            fn application_error(&self) -> AgentStream<Result<String, String>> {
                let (mut writer, stream) = AgentStream::new();
                spawn_local(async move {
                    writer.write_all([Ok("before".into()), Err("recoverable".into()), Ok("after".into())]).await.unwrap();
                });
                stream
            }
            fn malformed(&self) -> AgentStream<u32> {
                let (mut writer, stream) = golem_rust::schema::wit::new_schema_value_stream();
                spawn_local(async move {
                    let _ = writer.write_one(golem_rust::schema::wit::wire::SchemaValueTree {
                        value_nodes: vec![], root: 0,
                    }).await;
                });
                AgentStream::from_raw(stream)
            }
            async fn consume(&self, input: AgentStream<Item>) -> String {
                input.collect().await.unwrap().into_iter().map(|item| item.label).collect::<Vec<_>>().join(",")
            }
            async fn hold(&self, input: AgentStream<Item>) -> String {
                let _input = input;
                std::future::pending().await
            }
            fn drop_input(&self, input: AgentStream<Item>) -> String { drop(input); "dropped".into() }
            fn status(&self) -> String { "ready".into() }
        }
    "#}).unwrap();
    let scala_dir = ctx.cwd_path_join("consumer/src/main/scala");
    // Replace the scaffold's counter with the consumer under test.
    std::fs::remove_dir_all(&scala_dir).unwrap();
    fs::create_dir_all(scala_dir.join("consumer")).unwrap();
    fs::write_str(scala_dir.join("consumer/StreamConsumer.scala"), indoc! {r#"
        package consumer
        import golem.BaseAgent
        import golem.runtime.annotations.{agentDefinition, agentImplementation}
        import golem.schema.AgentStream
        import golem.bridge.client.stream_provider.{StreamProviderClient, Item, Bundle}
        import scala.concurrent.Future
        import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

        @agentDefinition()
        trait StreamConsumer extends BaseAgent {
          class Id(val name: String)
          def run(): Future[String]
          def nested(): Future[String]
          def cancel(): Future[String]
          def recoverable(): Future[String]
          def fatal(): Future[String]
        }
        @agentImplementation()
        final class StreamConsumerImpl(private val name: String) extends StreamConsumer {
          private def stream[A](values: List[A]): AgentStream[A] = {
            var remaining = values
            AgentStream.fromPull(() => {
              val result = remaining.headOption
              remaining = remaining.drop(1)
              Future.successful(result)
            })
          }
          private def collect[A](input: AgentStream[A]): Future[List[A]] =
            input.pull().flatMap {
              case None => input.close().map(_ => Nil)
              case Some(value) => collect(input).map(value :: _)
            }
          def run(): Future[String] = {
            val first = StreamProviderClient.get(name + "-first")
            val second = StreamProviderClient.get(name + "-second")
            val item = Item("root", List(Item("child", Nil)))
            for {
              echoed <- first.exchange(stream(List(item)))
              forwarded <- second.exchange(echoed)
              values <- collect(forwarded)
              produced <- first.produce()
              consumed <- second.consume(produced)
              bundle <- first.forward(Bundle(Some(stream(List(item))), List(stream(List(item)), stream(Nil))))
              optional <- collect(bundle.optional.get)
              siblings <- Future.sequence(bundle.siblings.map(collect))
              dropped <- first.dropInput(stream(List(item)))
              cancelable = first.produce.cancelable()
              cancelOutput <- cancelable._1
              _ <- cancelOutput.close()
              ready <- first.status()
            } yield {
              require(values == List(item) && consumed == "remote")
              require(optional == List(item) && siblings == List(List(item), Nil))
              require(dropped == "dropped" && ready == "ready")
              "scala-streams-ok"
            }
          }
          def nested(): Future[String] = {
            val provider = StreamProviderClient.get(name + "-nested")
            val item = Item("nested", List(Item("child", Nil)))
            for {
              outer <- provider.nested(stream(List(stream(List(item)))))
              inner <- outer.pull().map(_.get)
              values <- collect(inner)
              end <- outer.pull()
              _ <- outer.close()
              ready <- provider.status()
            } yield {
              require(values == List(item) && end.isEmpty && ready == "ready")
              "scala-nested-ok"
            }
          }
          def cancel(): Future[String] = {
            val provider = StreamProviderClient.get(name + "-cancel")
            val (result, token) = provider.hold.cancelable(stream(List(Item("cancel", Nil))))
            token.cancel()
            result.map(_ => false).recover { case _ => true }.map { cancelled =>
              require(cancelled)
              "scala-cancel-ok"
            }
          }
          def recoverable(): Future[String] = {
            val provider = StreamProviderClient.get(name + "-recoverable")
            for {
              input <- provider.applicationError()
              values <- collect(input)
              ready <- provider.status()
            } yield {
              require(values == List(Right("before"), Left("recoverable"), Right("after")))
              require(ready == "ready")
              "scala-recoverable-ok"
            }
          }
          def fatal(): Future[String] = {
            val provider = StreamProviderClient.get(name + "-fatal")
            provider.malformed().flatMap(collect).map(_ => "unexpected-clean-eof")
          }
        }
    "#}).unwrap();
    // Guest bridge sources belong to the consumer, not the provider's discovery input.
    let build_path = ctx.cwd_path_join("scala_guest_streams_consumer.sbt");
    let mut build = fs::read_to_string(&build_path).unwrap();
    build.push_str("\n  .settings(Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value / \"golem-temp/bridge-sdk/scala/internal/stream-provider-guest-client/src/main/scala\")\n");
    fs::write_str(build_path, build).unwrap();
    let output = ctx.cli([flag::YES, cmd::BUILD]).await;
    assert!(output.success_or_dump());
    let output = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(output.success_or_dump());
    ctx
}

#[test]
#[timeout("20 minutes")]
async fn test_scala_agent_guest_streams_e2e() {
    let ctx = deployed_scala_streams_context().await;
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "StreamConsumer(\"test\")",
            "run",
        ])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("scala-streams-ok"));
    for (method, expected) in [("cancel", "scala-cancel-ok"), ("nested", "scala-nested-ok")] {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            ctx.cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                "StreamConsumer(\"test\")",
                method,
            ]),
        )
        .await
        .expect("streaming invocation did not complete within two minutes");
        assert!(output.success_or_dump());
        assert!(output.stdout_contains(expected));
    }
}

#[test]
#[timeout("20 minutes")]
async fn test_scala_agent_guest_stream_producer_errors_e2e() {
    let ctx = deployed_scala_streams_context().await;
    let recoverable = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        ctx.cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "StreamConsumer(\"errors\")",
            "recoverable",
        ]),
    )
    .await
    .expect("recoverable result-item invocation timed out");
    assert!(recoverable.success_or_dump());
    assert!(recoverable.stdout_contains("scala-recoverable-ok"));

    let fatal = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        ctx.cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "StreamConsumer(\"fatal\")",
            "fatal",
        ]),
    )
    .await
    .expect("fatal producer invocation timed out");
    assert!(
        !fatal.success(),
        "malformed producer unexpectedly completed successfully"
    );
    assert!(
        !fatal.stdout_contains("unexpected-clean-eof"),
        "fatal producer was converted to clean EOF"
    );
    assert!(
        fatal.stderr_contains("durable stream ended with error context"),
        "expected a fatal stream operation failure: {}",
        fatal.stderr().collect::<Vec<_>>().join("\n")
    );
}

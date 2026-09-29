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

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

/// A Go agent calls a Rust agent's stream-bearing methods through its generated
/// guest client: a stream argument, a returned stream, streams inside an
/// option and a list, and a stream of streams.
#[test]
#[timeout("20 minutes")]
async fn test_go_generated_guest_streams_e2e() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("go-stream-bridge")).unwrap();
    ctx.cd("go-stream-bridge");
    for (template, component) in [("rust", "provider"), ("go", "consumer")] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                template,
                flag::COMPONENT_NAME,
                &format!("go-stream-bridge:{component}"),
            ])
            .await;
        assert!(outputs.success_or_dump());
    }
    // Written whole: both templates' CounterAgents are replaced below, so the
    // HTTP API deployment the templates declare for them must go too.
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: go-stream-bridge
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          go-stream-bridge:provider:
            dir: provider
            templates: rust
          go-stream-bridge:consumer:
            dir: consumer
            templates: go
            dependencies:
              agents:
                - go-stream-bridge:provider/StreamProvider
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("provider/src/counter_agent.rs"),
        indoc! {r#"
        use golem_rust::{agent_definition, agent_implementation, FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};
        use golem_rust::agentic::{AgentStream, spawn_local};

        #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
        pub struct StreamItem { pub label: String, pub children: Vec<StreamItem> }

        #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
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
        struct StreamProviderImpl { _name: String }
        #[agent_implementation]
        impl StreamProvider for StreamProviderImpl {
            fn new(name: String) -> Self { Self { _name: name } }
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
    "#},
    )
    .unwrap();

    // The Go template's own CounterAgent is replaced by the consumer.
    let consumer = ctx.cwd_path_join("consumer");
    let go_mod = consumer.join("go.mod");
    let module = fs::read_to_string(&go_mod)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the consumer's go.mod names its module");
    fs::remove(consumer.join("agents/counter")).unwrap();
    fs::write_str(
        consumer.join("agents/consumer/consumer.go"),
        indoc! {r#"
            package consumer

            import "github.com/golemcloud/golem/sdks/go/golem"

            type ID struct{ Name string }

            var Agent = golem.DefineAgent[ID](golem.Spec{Name: "StreamConsumer"})

            var Run = Agent.Method[golem.Unit, string]("run")
        "#},
    )
    .unwrap();
    fs::write_str(
        consumer.join("agents/consumer/impl/impl.go"),
        formatdoc! {r#"
            package impl

            import (
            	"fmt"

            	"{module}/agents/consumer"

            	provider "golem.local/bridge/stream-provider-guest-client"

            	"github.com/golemcloud/golem/sdks/go/core/values"
            	"github.com/golemcloud/golem/sdks/go/golem"
            )

            type state struct{{ name string }}

            var agent = consumer.Agent.Implement(func(id consumer.ID) *state {{ return &state{{name: id.Name}} }})

            func check(ok bool, format string, args ...any) {{
            	if !ok {{
            		panic(fmt.Sprintf(format, args...))
            	}}
            }}

            func next[T any](s golem.AgentStream[T]) (T, bool) {{
            	v, ok, err := s.Next()
            	check(err == nil, "read: %v", err)
            	return v, ok
            }}

            func init() {{
            	agent.Handle(consumer.Run, func(ctx *golem.Context[state], _ golem.Unit) string {{
            		remote := provider.GetStreamProvider(provider.StreamProviderId{{Name: ctx.State.name}})

            		total := remote.Consume(golem.StreamOf[int8](1, 2, 3))
            		check(total == 6, "consume returned %d", total)

            		output := remote.Produce()
            		forwarded := remote.Forward(provider.StreamBundle{{Optional: values.Some(output)}})
            		optional, some := forwarded.Optional.Get()
            		check(some, "forward lost the optional stream")
            		item, ok := next(optional)
            		check(ok && item.Label == "remote", "first item: %+v %v", item, ok)
            		check(len(item.Children) == 1 && item.Children[0].Label == "child", "children: %+v", item.Children)
            		_, ok = next(optional)
            		check(!ok, "the produced stream did not end")

            		for count := 0; count < 4; count++ {{
            			siblings := make([]golem.AgentStream[provider.StreamItem], 0, count)
            			for range count {{
            				siblings = append(siblings, remote.Produce())
            			}}
            			returned := remote.Forward(provider.StreamBundle{{Siblings: siblings}})
            			check(len(returned.Siblings) == count, "forwarded %d of %d siblings", len(returned.Siblings), count)
            			for _, s := range returned.Siblings {{
            				check(s.Close() == nil, "close a sibling")
            			}}
            		}}

            		outer := golem.ProduceStream(func(w *golem.AgentStreamWriter[golem.AgentStream[provider.StreamItem]]) error {{
            			return w.Write(golem.StreamOf(provider.StreamItem{{Label: "nested"}}))
            		}})
            		returned := remote.Nested(outer)
            		inner, ok := next(returned)
            		check(ok, "the nested stream is empty")
            		item, ok = next(inner)
            		check(ok && item.Label == "nested", "nested item: %+v %v", item, ok)
            		_, ok = next(inner)
            		check(!ok, "the inner stream did not end")
            		_, ok = next(returned)
            		check(!ok, "the outer stream did not end")

            		return "ok:" + remote.Status()
            	}})
            }}
        "#},
    )
    .unwrap();
    let main_go = consumer.join("main.go");
    fs::write_str(
        &main_go,
        fs::read_to_string(&main_go).unwrap().replace(
            &format!("{module}/agents/counter/impl"),
            &format!("{module}/agents/consumer/impl"),
        ),
    )
    .unwrap();
    fs::write_str(
        &go_mod,
        fs::read_to_string(&go_mod).unwrap()
            + indoc! {r#"

                require golem.local/bridge/stream-provider-guest-client v0.0.0

                replace golem.local/bridge/stream-provider-guest-client => ../golem-temp/bridge-sdk/go/internal/stream-provider-guest-client
            "#},
    )
    .unwrap();

    assert!(ctx.cli([cmd::BUILD]).await.success_or_dump());
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("StreamConsumer(\"{}\")", Uuid::new_v4()),
            "run",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("ok:ready"));
}

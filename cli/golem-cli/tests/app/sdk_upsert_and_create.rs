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

//! An agent of the Scala, MoonBit and Effect SDKs upserts the template's `CounterAgent` twice
//! and creates two counters without a key from the same constructor parameters. `check` returns
//! the count of the second upsert, the counts of the two created counters, and whether the two
//! upserts share one ID, the created counters differ from it, and they differ from each other.

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::fs;
use indoc::indoc;
use std::path::{Path, PathBuf};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

const EXPECTED: &str = "2|1|1|true|true|true";

async fn deploy_and_check(template: &str, write_caller: impl FnOnce(&TestContext)) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let app = format!("{template}-upsert-create");
    assert!(
        ctx.cli([flag::YES, cmd::NEW, &app, flag::TEMPLATE, template])
            .await
            .success_or_dump()
    );
    ctx.cd(&app);
    write_caller(&ctx);
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let counter = format!("\"{}\"", Uuid::new_v4());
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            r#"IdentityCaller("caller")"#,
            "check",
            &counter,
        ])
        .await;
    assert!(outputs.success_or_dump());
    let result = outputs.stdout_text();
    assert!(result.contains(EXPECTED), "{template}: {result}");
}

/// The file named `name` in the application, outside its build outputs.
fn find_source(root: &Path, name: &str) -> PathBuf {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let file_name = path.file_name().unwrap().to_string_lossy();
            if path.is_dir() {
                if !["golem-temp", "target", "node_modules", "_build", ".bsp"]
                    .contains(&file_name.as_ref())
                {
                    pending.push(path);
                }
            } else if file_name == name {
                return path;
            }
        }
    }
    panic!("no {name} under {}", root.display())
}

#[test]
#[timeout("20 minutes")]
async fn scala_agent_upserts_one_agent_and_creates_distinct_agents_without_a_key() {
    deploy_and_check("scala", |ctx| {
        let counter = find_source(ctx.cwd_path(), "CounterAgent.scala");
        let package = fs::read_to_string(&counter)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        fs::write_str(
            counter.with_file_name("IdentityCaller.scala"),
            format!(
                "{package}\n{}",
                indoc! {r#"
                    import golem.BaseAgent
                    import golem.runtime.annotations.{agentDefinition, agentImplementation}

                    import scala.annotation.unused
                    import scala.concurrent.Future
                    import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

                    @agentDefinition()
                    trait IdentityCaller extends BaseAgent {
                      class Id(val name: String)
                      def check(counter: String): Future[String]
                    }

                    @agentImplementation()
                    final class IdentityCallerImpl(@unused private val name: String) extends IdentityCaller {
                      override def check(counter: String): Future[String] = {
                        val first = CounterAgentClient.upsert(counter)
                        val again = CounterAgentClient.upsert(counter)
                        for {
                          _       <- first.increment()
                          count   <- again.increment()
                          a       <- CounterAgentClient.create(counter)
                          b       <- CounterAgentClient.create(counter)
                          aCount  <- a.increment()
                          bCount  <- b.increment()
                          firstId <- first.agentId()
                          againId <- again.agentId()
                          aId     <- a.agentId()
                          bId     <- b.agentId()
                        } yield List(
                          count,
                          aCount,
                          bCount,
                          firstId == againId,
                          aId != firstId && bId != firstId,
                          aId != bId
                        ).mkString("|")
                      }
                    }
                "#}
            ),
        )
        .unwrap();
    })
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn moonbit_agent_upserts_one_agent_and_creates_distinct_agents_without_a_key() {
    deploy_and_check("moonbit", |ctx| {
        let counter = find_source(ctx.cwd_path(), "counter.mbt");
        fs::write_str(
            counter.with_file_name("identity_caller.mbt"),
            indoc! {r#"
                ///|
                #derive.agent
                struct IdentityCaller {
                  name : String
                }

                ///|
                fn IdentityCaller::new(name : String) -> IdentityCaller {
                  { name, }
                }

                ///|
                pub async fn IdentityCaller::check(_self : Self, counter : String) -> String {
                  let first = CounterAgentClient::upsert(counter)
                  ignore(first.increment())
                  let again = CounterAgentClient::upsert(counter)
                  let count = again.increment()
                  let a = CounterAgentClient::create(counter)
                  let b = CounterAgentClient::create(counter)
                  let a_count = a.increment()
                  let b_count = b.increment()
                  let first_id = first.agent_id()
                  let same = first_id == again.agent_id()
                  let a_id = a.agent_id()
                  let b_id = b.agent_id()
                  let not_upserted = a_id != first_id && b_id != first_id
                  let distinct = a_id != b_id
                  "\{count}|\{a_count}|\{b_count}|\{same}|\{not_upserted}|\{distinct}"
                }
            "#},
        )
        .unwrap();
    })
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn effect_agent_upserts_one_agent_and_creates_distinct_agents_without_a_key() {
    deploy_and_check("effect", |ctx| {
        let main = find_source(ctx.cwd_path(), "main.ts");
        let mut source = fs::read_to_string(&main).unwrap();
        source.push_str("import \"./identity-caller.js\";\n");
        fs::write_str(&main, source).unwrap();
        fs::write_str(
            main.with_file_name("identity-caller.ts"),
            indoc! {r#"
                import { Effect, Schema } from "effect";
                import { defineAgent, method } from "@golemcloud/effect-golem";
                import { CounterAgent } from "./counter-agent.js";

                export const IdentityCaller = defineAgent({
                  name: "IdentityCaller",
                  id: { name: Schema.String },
                  methods: {
                    check: method({
                      input: { counter: Schema.String },
                      success: Schema.String,
                    }),
                  },
                }).implement({
                  init: ({ name }) => Effect.succeed(name),
                  methods: () => ({
                    check: ({ counter }) =>
                      Effect.gen(function* () {
                        const first = yield* CounterAgent.client.upsert({ name: counter });
                        yield* first.increment({});
                        const again = yield* CounterAgent.client.upsert({ name: counter });
                        const count = yield* again.increment({});
                        const a = yield* CounterAgent.client.create({ name: counter });
                        const b = yield* CounterAgent.client.create({ name: counter });
                        const aCount = yield* a.increment({});
                        const bCount = yield* b.increment({});
                        const [firstId, againId, aId, bId] = yield* Effect.all([
                          first.agentId,
                          again.agentId,
                          a.agentId,
                          b.agentId,
                        ]);
                        return [
                          count,
                          aCount,
                          bCount,
                          firstId.uuid === againId.uuid,
                          aId.uuid !== firstId.uuid && bId.uuid !== firstId.uuid,
                          aId.uuid !== bId.uuid,
                        ].join("|");
                      }).pipe(Effect.orDie, Effect.scoped),
                  }),
                });
            "#},
        )
        .unwrap();
    })
    .await;
}

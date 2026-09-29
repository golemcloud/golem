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
use crate::app::{Output, TestContext, cmd, flag};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

async fn new_rust_app(ctx: &mut TestContext) {
    ctx.start_server().await;
    let output = ctx
        .cli([flag::YES, cmd::NEW, "secrets-app", flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd("secrets-app");
}

fn single_json(output: &Output) -> serde_json::Value {
    output
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .next()
        .expect("command produced no JSON output")
}

#[test]
#[timeout("5m")]
async fn secret_create_update_and_delete(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    new_rust_app(&mut ctx).await;

    let output = ctx
        .cli([
            cmd::SECRET,
            "create",
            "api.key",
            "--type",
            "String",
            "--value",
            "first",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let created = single_json(&output);
    assert_eq!(created["$type"], "secret.create");
    assert_eq!(created["action"], "created");

    let output = ctx
        .cli([
            cmd::SECRET,
            "create",
            "api.key",
            "--type",
            "String",
            "--value",
            "again",
        ])
        .await;
    assert!(!output.success());
    assert!(
        output.stdout_contains("--update-existing"),
        "{}",
        output.stdout_text()
    );

    let output = ctx
        .cli([
            cmd::SECRET,
            "create",
            "api.key",
            "--type",
            "String",
            "--value",
            "second",
            "--update-existing",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let updated = single_json(&output);
    assert_eq!(updated["action"], "updated");
    assert_eq!(updated["id"], created["id"]);

    let output = ctx
        .cli([flag::SHOW_SECRETS, cmd::SECRET, cmd::GET, "api.key"])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("second"));

    let output = ctx
        .cli_with_input(
            [cmd::SECRET, "update", "api.key", "--value-stdin"],
            b"third\n",
        )
        .await;
    assert!(output.success(), "{}", output.stderr_text());

    let output = ctx
        .cli([flag::SHOW_SECRETS, cmd::SECRET, cmd::GET, "api.key"])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("third"));

    let output = ctx.cli([cmd::SECRET, "update", "api.key"]).await;
    assert!(!output.success());
    assert!(
        output.stderr_contains("--value, --value-stdin or --unset"),
        "{}",
        output.stderr_text()
    );

    let output = ctx
        .cli([
            cmd::SECRET,
            "update",
            "api.key",
            "--unset",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let unset = single_json(&output);
    assert_eq!(unset["$type"], "secret.update");
    assert!(unset["secretValue"].is_null());

    let output = ctx.cli([cmd::SECRET, "delete", "api.key"]).await;
    assert!(!output.success());
    let output = ctx.cli([cmd::SECRET, cmd::GET, "api.key"]).await;
    assert!(output.success_or_dump());

    let output = ctx.cli([flag::YES, cmd::SECRET, "delete", "api.key"]).await;
    assert!(output.success_or_dump());
    let output = ctx.cli([cmd::SECRET, cmd::GET, "api.key"]).await;
    assert!(!output.success());
}

#[test]
#[timeout("5m")]
async fn secret_create_update_existing_with_type_change(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    new_rust_app(&mut ctx).await;

    let output = ctx
        .cli([
            cmd::SECRET,
            "create",
            "retries",
            "--type",
            "String",
            "--value",
            "three",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let created = single_json(&output);

    let output = ctx
        .cli([
            cmd::SECRET,
            "create",
            "retries",
            "--type",
            "u32",
            "--value",
            "3",
            "--update-existing",
        ])
        .await;
    assert!(!output.success());
    assert!(
        output.stdout_contains("different type"),
        "{}",
        output.stdout_text()
    );

    let output = ctx
        .cli([
            flag::YES,
            cmd::SECRET,
            "create",
            "retries",
            "--type",
            "u32",
            "--value",
            "3",
            "--update-existing",
            "--replace-on-type-change",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let replaced = single_json(&output);
    assert_eq!(replaced["action"], "replaced");
    assert_ne!(replaced["id"], created["id"]);
}

#[test]
#[timeout("5m")]
async fn create_update_existing_for_retry_policy_and_resource(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    new_rust_app(&mut ctx).await;

    let predicate = r#"{ "propIn": { "property": "status-code", "values": [502, 503, 504] } }"#;
    let policy = r#"{ "countBox": { "maxRetries": 5, "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } } } }"#;
    let retry_policy_create = |priority: &'static str, update_existing: bool| {
        let mut args = vec![
            cmd::RETRY_POLICY,
            "create",
            "http-transient",
            "--priority",
            priority,
            "--predicate",
            predicate,
            "--policy",
            policy,
            flag::FORMAT,
            "json",
        ];
        if update_existing {
            args.push("--update-existing");
        }
        args
    };

    let output = ctx.cli(retry_policy_create("10", false)).await;
    assert!(output.success_or_dump());
    assert_eq!(single_json(&output)["action"], "created");

    let output = ctx.cli(retry_policy_create("20", false)).await;
    assert!(!output.success());
    assert!(
        output.stderr_contains("--update-existing"),
        "{}",
        output.stderr_text()
    );

    let output = ctx.cli(retry_policy_create("20", true)).await;
    assert!(output.success_or_dump());
    let updated = single_json(&output);
    assert_eq!(updated["action"], "updated");
    assert_eq!(updated["priority"], 20);

    let resource_create = |value: &'static str, update_existing: bool| {
        let mut args = vec![
            cmd::RESOURCE,
            "create",
            "concurrent-jobs",
            "--limit",
            value,
            flag::FORMAT,
            "json",
        ];
        if update_existing {
            args.push("--update-existing");
        }
        args
    };

    let output = ctx
        .cli(resource_create(
            r#"{"type":"Concurrency","value":4}"#,
            false,
        ))
        .await;
    assert!(output.success_or_dump());
    assert_eq!(single_json(&output)["action"], "created");

    let output = ctx
        .cli(resource_create(
            r#"{"type":"Concurrency","value":8}"#,
            false,
        ))
        .await;
    assert!(!output.success());
    assert!(
        output.stderr_contains("--update-existing"),
        "{}",
        output.stderr_text()
    );

    let output = ctx
        .cli(resource_create(r#"{"type":"Concurrency","value":8}"#, true))
        .await;
    assert!(output.success_or_dump());
    assert_eq!(single_json(&output)["action"], "updated");
}

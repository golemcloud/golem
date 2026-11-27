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
use desert_rust::BinaryCodec;
use golem_common::schema::SchemaValue;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
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
    assert!(output.success_or_dump());
    output
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .next()
        .expect("command produced no JSON output")
}

fn assert_management_output(text: &str) {
    for forbidden in [
        "secretValue",
        "cli-secret-first",
        "cli-secret-second",
        "cli-secret-third",
        "***",
    ] {
        assert!(
            !text.contains(forbidden),
            "management output contains {forbidden}: {text}"
        );
    }
}

fn assert_metadata(secret: &serde_json::Value, has_value: bool, destinations: &[&str]) {
    assert!(secret.get("secretValue").is_none());
    assert_eq!(secret["hasValue"], has_value);
    let mut actual: Vec<&str> = secret["destinations"]
        .as_array()
        .expect("missing destinations")
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    actual.sort_unstable();
    let mut expected = destinations.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

// The management API deliberately has no value read operation. Read the internal revision
// payload from the test server's SQLite database to verify that updates preserve the value.
#[derive(BinaryCodec)]
#[desert(evolution())]
struct StoredSecretRevision {
    secret_value: Option<SchemaValue>,
}

async fn assert_stored_value(
    ctx: &TestContext,
    secret: &serde_json::Value,
    expected: Option<SchemaValue>,
) {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(ctx.data_dir.path().join("registry.db"))
            .read_only(true),
    )
    .await
    .unwrap();
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT agent_secret_revision_data FROM agent_secret_revisions WHERE agent_secret_id = ? AND revision_id = ?",
    )
    .bind(secret["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(secret["revision"].as_i64().unwrap())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let stored: StoredSecretRevision = desert_rust::deserialize(&bytes).unwrap();
    assert_eq!(stored.secret_value, expected);
}

#[test]
#[timeout("5m")]
async fn secret_create_update_and_delete(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    new_rust_app(&mut ctx).await;

    for show_secrets in [false, true] {
        let mut visibility = Vec::new();
        if show_secrets {
            visibility.push(flag::SHOW_SECRETS);
        }
        let destinations = ["https://api.example.com", "https://*.example.org"];
        let output = ctx
            .cli_with_input(
                visibility.iter().copied().chain([
                    cmd::SECRET,
                    "create",
                    "api.key",
                    "--type",
                    "String",
                    "--value-stdin",
                    "--destination",
                    destinations[0],
                    "--destination",
                    destinations[1],
                    flag::FORMAT,
                    "json",
                ]),
                b"cli-secret-first\n",
            )
            .await;
        assert!(output.success(), "{}", output.stderr_text());
        assert_management_output(&output.stdout_text());
        let created: serde_json::Value = serde_json::from_slice(output.stdout()).unwrap();
        assert_eq!(created["$type"], "secret.create");
        assert_eq!(created["action"], "created");
        assert_metadata(&created, true, &destinations);
        assert_stored_value(
            &ctx,
            &created,
            Some(SchemaValue::String("cli-secret-first".into())),
        )
        .await;

        for format in ["text", "yaml"] {
            for (command, value) in [
                ("create", "cli-secret-first"),
                ("update", "cli-secret-second"),
            ] {
                let mut args = visibility.clone();
                args.extend([cmd::SECRET, command, "format.key", "--value", value]);
                if command == "create" {
                    args.extend(["--type", "String"]);
                }
                args.extend(["--destination", destinations[0], flag::FORMAT, format]);
                let output = ctx.cli(args).await;
                assert!(output.success_or_dump());
                assert_management_output(&output.stdout_text());
                assert!(output.stdout_contains(destinations[0]));
                if format == "yaml" {
                    let secret = serde_yaml::from_str(&output.stdout_text()).unwrap();
                    assert_metadata(&secret, true, &[destinations[0]]);
                }

                let output = ctx
                    .cli([cmd::SECRET, cmd::GET, "format.key", flag::FORMAT, "json"])
                    .await;
                let secret = single_json(&output);
                assert_metadata(&secret, true, &[destinations[0]]);
                assert_stored_value(&ctx, &secret, Some(SchemaValue::String(value.into()))).await;
            }
            let output = ctx
                .cli([flag::YES, cmd::SECRET, "delete", "format.key"])
                .await;
            assert!(output.success_or_dump());
        }

        for show in [false, true] {
            for command in [cmd::GET, "list"] {
                for format in ["json", "yaml", "text"] {
                    let mut args = vec![cmd::SECRET, command];
                    if command == cmd::GET {
                        args.push("api.key");
                    }
                    args.extend([flag::FORMAT, format]);
                    if show {
                        args.push(flag::SHOW_SECRETS);
                    }
                    let output = ctx.cli(args).await;
                    assert!(output.success_or_dump());
                    assert_management_output(&output.stdout_text());
                    if format != "text" {
                        let value = if format == "json" {
                            single_json(&output)
                        } else {
                            serde_yaml::from_str(&output.stdout_text()).unwrap()
                        };
                        let secret = if command == cmd::GET {
                            &value
                        } else {
                            &value["secrets"][0]
                        };
                        assert_metadata(secret, true, &destinations);
                        assert_eq!(secret["revision"], created["revision"]);
                    } else {
                        for destination in destinations {
                            assert!(output.stdout_contains(destination));
                        }
                    }
                }
            }
        }

        let output = ctx
            .cli([
                cmd::SECRET,
                cmd::GET,
                "api.key",
                "--destination",
                "https://other.example.com",
            ])
            .await;
        assert!(!output.success());

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
            .cli(visibility.iter().copied().chain([
                cmd::SECRET,
                "create",
                "api.key",
                "--type",
                "String",
                "--value",
                "cli-secret-second",
                "--update-existing",
                "--destination",
                "https://updated.example.com",
                flag::FORMAT,
                "json",
            ]))
            .await;
        assert!(output.success_or_dump());
        let updated = single_json(&output);
        assert_management_output(&output.stdout_text());
        assert_eq!(updated["action"], "updated");
        assert_eq!(updated["id"], created["id"]);
        assert_metadata(&updated, true, &["https://updated.example.com"]);
        assert_eq!(
            updated["revision"].as_i64().unwrap(),
            created["revision"].as_i64().unwrap() + 1
        );
        assert_stored_value(
            &ctx,
            &updated,
            Some(SchemaValue::String("cli-secret-second".into())),
        )
        .await;

        let output = ctx
            .cli_with_input(
                visibility.iter().copied().chain([
                    cmd::SECRET,
                    "update",
                    "api.key",
                    "--destination",
                    destinations[0],
                    "--destination",
                    destinations[1],
                    flag::FORMAT,
                    "json",
                ]),
                b"",
            )
            .await;
        assert!(output.success(), "{}", output.stderr_text());
        assert_management_output(&output.stdout_text());
        let destination_only: serde_json::Value = serde_json::from_slice(output.stdout()).unwrap();
        assert_metadata(&destination_only, true, &destinations);
        assert_eq!(
            destination_only["revision"].as_i64().unwrap(),
            updated["revision"].as_i64().unwrap() + 1
        );
        assert_stored_value(
            &ctx,
            &destination_only,
            Some(SchemaValue::String("cli-secret-second".into())),
        )
        .await;

        let output = ctx
            .cli_with_input(
                visibility.iter().copied().chain([
                    cmd::SECRET,
                    "update",
                    "api.key",
                    "--value-stdin",
                    flag::FORMAT,
                    "json",
                ]),
                b"cli-secret-third\n",
            )
            .await;
        assert!(output.success(), "{}", output.stderr_text());
        assert_management_output(&output.stdout_text());
        let value_only: serde_json::Value = serde_json::from_slice(output.stdout()).unwrap();
        assert_metadata(&value_only, true, &destinations);
        assert_stored_value(
            &ctx,
            &value_only,
            Some(SchemaValue::String("cli-secret-third".into())),
        )
        .await;

        let output = ctx
            .cli_with_input(
                visibility.iter().copied().chain([
                    cmd::SECRET,
                    "update",
                    "api.key",
                    "--value-stdin",
                    "--destination",
                    "https://combined.example.com",
                    flag::FORMAT,
                    "json",
                ]),
                b"cli-secret-first\n",
            )
            .await;
        assert!(output.success(), "{}", output.stderr_text());
        assert_management_output(&output.stdout_text());
        let combined: serde_json::Value = serde_json::from_slice(output.stdout()).unwrap();
        assert_metadata(&combined, true, &["https://combined.example.com"]);
        assert_eq!(
            combined["revision"].as_i64().unwrap(),
            value_only["revision"].as_i64().unwrap() + 1
        );
        assert_stored_value(
            &ctx,
            &combined,
            Some(SchemaValue::String("cli-secret-first".into())),
        )
        .await;

        let output = ctx.cli([cmd::SECRET, "update", "api.key"]).await;
        assert!(!output.success());
        assert!(
            output.stderr_contains("--destination"),
            "{}",
            output.stderr_text()
        );

        let output = ctx
            .cli(visibility.iter().copied().chain([
                cmd::SECRET,
                "update",
                "api.key",
                "--unset",
                flag::FORMAT,
                "json",
            ]))
            .await;
        assert!(output.success_or_dump());
        let unset = single_json(&output);
        assert_management_output(&output.stdout_text());
        assert_eq!(unset["$type"], "secret.update");
        assert_metadata(&unset, false, &["https://combined.example.com"]);
        assert_stored_value(&ctx, &unset, None).await;

        let output = ctx.cli([cmd::SECRET, "delete", "api.key"]).await;
        assert!(!output.success());
        let output = ctx.cli([cmd::SECRET, cmd::GET, "api.key"]).await;
        assert!(output.success_or_dump());
        assert_management_output(&output.stdout_text());
        for command in [cmd::GET, "list"] {
            let mut args = visibility.clone();
            args.extend([cmd::SECRET, command]);
            if command == cmd::GET {
                args.push("api.key");
            }
            args.extend([flag::FORMAT, "json"]);
            let output = ctx.cli(args).await;
            let value = single_json(&output);
            assert_management_output(&output.stdout_text());
            let secret = if command == cmd::GET {
                &value
            } else {
                &value["secrets"][0]
            };
            assert_metadata(secret, false, &["https://combined.example.com"]);
            assert_eq!(secret["revision"], unset["revision"]);
        }

        let output = ctx.cli([flag::YES, cmd::SECRET, "delete", "api.key"]).await;
        assert!(output.success_or_dump());
        let output = ctx.cli([cmd::SECRET, cmd::GET, "api.key"]).await;
        assert!(!output.success());
    }
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
            "cli-secret-first",
            "--destination",
            "https://original.example.com",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let created = single_json(&output);
    assert_management_output(&output.stdout_text());
    assert_metadata(&created, true, &["https://original.example.com"]);
    assert_stored_value(
        &ctx,
        &created,
        Some(SchemaValue::String("cli-secret-first".into())),
    )
    .await;

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
            "--destination",
            "ftp://x.com",
        ])
        .await;
    assert!(!output.success());
    let output = ctx
        .cli([cmd::SECRET, cmd::GET, "retries", flag::FORMAT, "json"])
        .await;
    let unchanged = single_json(&output);
    assert_eq!(unchanged["id"], created["id"]);
    assert_eq!(unchanged["revision"], created["revision"]);
    assert_metadata(&unchanged, true, &["https://original.example.com"]);
    assert_stored_value(
        &ctx,
        &unchanged,
        Some(SchemaValue::String("cli-secret-first".into())),
    )
    .await;

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
            flag::SHOW_SECRETS,
            cmd::SECRET,
            "create",
            "retries",
            "--type",
            "u32",
            "--value",
            "3",
            "--update-existing",
            "--replace-on-type-change",
            "--destination",
            "https://replacement.example.com",
            "--destination",
            "https://*.replacement.example.org",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let replaced = single_json(&output);
    assert_management_output(&output.stdout_text());
    assert_eq!(replaced["action"], "replaced");
    assert_ne!(replaced["id"], created["id"]);
    assert_metadata(
        &replaced,
        true,
        &[
            "https://replacement.example.com",
            "https://*.replacement.example.org",
        ],
    );
    assert_stored_value(&ctx, &replaced, Some(SchemaValue::U32(3))).await;
    let output = ctx
        .cli([cmd::SECRET, cmd::GET, "retries", flag::FORMAT, "json"])
        .await;
    let fetched = single_json(&output);
    assert_eq!(fetched["id"], replaced["id"]);
    assert_metadata(
        &fetched,
        true,
        &[
            "https://replacement.example.com",
            "https://*.replacement.example.org",
        ],
    );
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

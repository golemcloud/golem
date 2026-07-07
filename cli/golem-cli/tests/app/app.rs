use crate::Tracing;
use crate::app::{TestContext, check_component_metadata, cmd, flag, pattern};

use golem_cli::fs;
use golem_cli::model::GuestLanguage;
use golem_cli::versions;
use indoc::{formatdoc, indoc};
use serde_json::Value as JsonValue;
use std::path::Path;
use std::time::Duration;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, test, timeout};
use toml_edit::{DocumentMut, value};

inherit_test_dep!(Tracing);

#[test]
async fn app_help_in_empty_folder(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(pattern::HELP_USAGE));
    assert!(!outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    assert!(!outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
}

#[test]
async fn app_help_does_not_apply_manifest_upgrade(_tracing: &Tracing) {
    let app_name = "test-app-help-no-upgrade";

    let mut ctx = TestContext::new();
    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let old_manifest = indoc! {"
        manifestVersion: 1.5.0

        app: test-app-help-no-upgrade

        environments:
          local:
            server: local
            componentPresets: debug

        components:
          test-app-help-no-upgrade:ts-main:
            templates: ts
    "};
    fs::write_str(&manifest_path, old_manifest).unwrap();

    // Help runs (without --yes, in a non-interactive shell) must not plan or
    // apply manifest upgrades, while still showing application help
    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(pattern::HELP_USAGE));
    assert!(outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    assert!(!outputs.stderr_contains("Planned automatic application manifest upgrade"));
    assert!(!outputs.stderr_contains("This action requires confirmation"));
    assert!(!outputs.stdout_contains("Planned automatic application manifest upgrade"));

    assert_eq!(fs::read_to_string(&manifest_path).unwrap(), old_manifest);
}

#[test]
async fn app_new_with_many_components_and_then_help_in_app_folder(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            app_name,
            flag::TEMPLATE,
            "ts",
            flag::TEMPLATE,
            "rust",
        ])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "ts",
            flag::COMPONENT_NAME,
            "app:typescript",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "app:rust",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(pattern::HELP_USAGE));
    assert!(outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    assert!(outputs.stderr_contains("app:rust"));
    assert!(outputs.stderr_contains("app:typescript"));
    assert!(outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
    assert!(outputs.stderr_contains("npm-install"));
}

#[test]
async fn app_build_with_rust_component(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    // First build
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("Compiling test_app_name_rust_main v0.0.1"));

    check_component_metadata(
        &ctx.working_dir
            .join("golem-temp/agents/test_app_name_rust_main_debug.wasm"),
        "test-app-name:rust-main".to_string(),
        None,
    );

    // Rebuild - 1
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stdout_contains("Compiling test_app_name_rust_main v0.0.1"));

    // Rebuild - 2
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stdout_contains("Compiling test_app_name_rust_main v0.0.1"));

    // Rebuild - 3 - force, but cargo is smart to skip actual compile
    let outputs = ctx.cli([cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("Finished `dev` profile"));

    // Rebuild - 4
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stdout_contains("Compiling test_app_name_rust_main v0.0.1"));

    // Clean
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    // Rebuild - 5
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stdout_contains("Compiling test_app_name_rust_main v0.0.1"));
}

#[test]
async fn build_check(_tracing: &Tracing) {
    let app_name = "test-app-check-step";

    let mut ctx = TestContext::new();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            app_name,
            flag::TEMPLATE,
            "ts",
            flag::TEMPLATE,
            "rust",
        ])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    // Phase 1: baseline check on a freshly generated mixed TS+Rust app.
    let outputs = ctx.cli([cmd::BUILD, flag::STEP, "check"]).await;
    assert!(outputs.success_or_dump());

    let ts_component_dir = Path::new("ts-main");
    let rust_component_dir = Path::new("rust-main");

    let package_json_path = ctx.cwd_path_join("package.json");
    let mut package_json: JsonValue =
        serde_json::from_str(fs::read_to_string(&package_json_path).unwrap().as_str()).unwrap();

    package_json["dependencies"]["@golemcloud/golem-ts-sdk"] =
        JsonValue::String("0.0.1".to_string());
    fs::write_str(
        &package_json_path,
        serde_json::to_string_pretty(&package_json).unwrap(),
    )
    .unwrap();

    let tsconfig_path = ctx.cwd_path_join(ts_component_dir.join("tsconfig.json"));
    let mut tsconfig: JsonValue =
        serde_json::from_str(fs::read_to_string(&tsconfig_path).unwrap().as_str()).unwrap();

    tsconfig["compilerOptions"]["moduleResolution"] = JsonValue::String("node".to_string());
    fs::write_str(
        &tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap(),
    )
    .unwrap();

    let cargo_toml_path = ctx.cwd_path_join(rust_component_dir.join("Cargo.toml"));
    let mut cargo_toml: DocumentMut = fs::read_to_string(&cargo_toml_path)
        .unwrap()
        .parse()
        .unwrap();
    cargo_toml["dependencies"]["golem-rust"] = value("999.0.0");
    fs::write_str(&cargo_toml_path, cargo_toml.to_string()).unwrap();

    // Phase 2: intentionally break versions/settings and verify build check can auto-fix them.
    let outputs = ctx.cli([flag::YES, cmd::BUILD, flag::STEP, "check"]).await;
    assert!(outputs.success_or_dump());
    assert!(
        outputs.stdout_contains("Planned required changes for dependencies and configurations")
    );
    assert!(outputs.stdout_contains("Applying dependency and configuration updates"));

    fs::write_str(
        &package_json_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "app",
            // The fluent `ts` agents import `zod`; the build check treats zod as an
            // optional dependency (only validated when present), so keep it here —
            // wiping it would leave the restored app unable to resolve the import.
            "dependencies": { "zod": versions::ts_dep::ZOD },
            "devDependencies": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let mut cargo_toml: DocumentMut = fs::read_to_string(&cargo_toml_path)
        .unwrap()
        .parse()
        .unwrap();
    cargo_toml["dependencies"] = toml_edit::Item::Table(toml_edit::Table::default());
    cargo_toml["dependencies"]["golem-rust"] =
        toml_edit::value(toml_edit::InlineTable::from_iter([
            (
                "path",
                toml_edit::Value::from(
                    "/Users/noise64/workspace/golem-alt-00/sdks/rust/golem-rust",
                ),
            ),
            (
                "features",
                toml_edit::Value::Array({
                    let mut features = toml_edit::Array::default();
                    features.push("export_golem_agentic");
                    features
                }),
            ),
        ]));
    fs::write_str(&cargo_toml_path, cargo_toml.to_string()).unwrap();

    // Phase 3: wipe dependencies completely and verify check restores everything needed.
    let outputs = ctx.cli([flag::YES, cmd::BUILD, flag::STEP, "check"]).await;
    assert!(outputs.success_or_dump());
    assert!(
        outputs.stdout_contains("Planned required changes for dependencies and configurations")
    );
    assert!(outputs.stdout_contains("Applying dependency and configuration updates"));

    // Phase 4: full build should succeed after all auto-applied fixes.
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn app_new_language_hints(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([flag::YES, cmd::NEW, "dummy-app-name"]).await;
    assert!(!outputs.success());
    assert!(outputs.stdout_contains("Available languages:"));

    let languages_without_templates = GuestLanguage::iter()
        .filter(|language| !outputs.stdout_contains(format!("- {language}")))
        .collect::<Vec<_>>();

    assert!(
        languages_without_templates.is_empty(),
        "{:?}",
        languages_without_templates
    );
}

#[test]
async fn app_new_unpacks_embedded_bootstrap_skills(_tracing: &Tracing) {
    for (app_name, language) in [
        ("test-app-embedded-skills-ts", "ts"),
        ("test-app-embedded-skills-rust", "rust"),
        ("test-app-embedded-skills-scala", "scala"),
    ] {
        let mut ctx = TestContext::new();
        let outputs = ctx
            .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, language])
            .await;
        assert!(outputs.success_or_dump(), "language={language}");

        ctx.cd(app_name);

        assert!(
            ctx.cwd_path_join(".agents/skills/golem-new-project/SKILL.md")
                .exists(),
            "missing embedded bootstrap skill for {language}",
        );

        let claude_link = ctx.cwd_path_join(".claude");
        assert!(
            claude_link.is_symlink(),
            "missing .claude symlink for {language}",
        );
    }
}

#[test]
async fn completion(_tracing: &Tracing) {
    let ctx = TestContext::new();

    let outputs = ctx.cli([cmd::COMPLETION, "bash"]).await;
    assert!(outputs.success(), "bash");

    let outputs = ctx.cli([cmd::COMPLETION, "elvish"]).await;
    assert!(outputs.success(), "elvish");

    let outputs = ctx.cli([cmd::COMPLETION, "fish"]).await;
    assert!(outputs.success(), "fish");

    let outputs = ctx.cli([cmd::COMPLETION, "powershell"]).await;
    assert!(outputs.success(), "powershell");

    let outputs = ctx.cli([cmd::COMPLETION, "zsh"]).await;
    assert!(outputs.success(), "zsh");
}

#[test]
async fn ts_repl_interactive(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-repl-interactive";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("sample-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

            export const SampleAgent = defineAgent({
                name: 'SampleAgent',
                id: {
                    name: z.string(),
                    region: z.string(),
                    mode: z.enum(['fast', 'safe']),
                    complex: z.object({ a: z.number(), b: z.string() }).optional(),
                },
                methods: {
                    sampleMethod: method({
                        input: {},
                        returns: z.number(),
                        promptHint: 'Run sample method',
                        description: 'Runs the sample method and returns the new value',
                    }),
                },
            });

            export const SampleAgentImpl = SampleAgent.implement({
                init: ({ id }) => ({
                    name: id.name,
                    region: id.region,
                    mode: id.mode,
                    complex: id.complex ?? { a: 0, b: 'default' },
                    value: 0,
                }),
                methods: {
                    sampleMethod() {
                        this.value += 1;
                        return this.value;
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("main.ts")),
        indoc! {
            r#"
            import './counter-agent.js';
            import './sample-agent.js';
            "#
        },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    ctx.cli_interactive_repl_test(
        [
            cmd::REPL,
            flag::LANGUAGE,
            "ts",
            flag::YES,
            "--disable-stream",
        ],
        move |session| {
            let agent_type_info_regex = indoc! {
                r#"(?sx)
            Available\s+agent\s+client\s+types:
            .*
            (
                SampleAgent\.get\(
                    name:\s*string,\s*
                    region:\s*string,\s*
                    mode:\s*"fast"\s*\|\s*"safe",\s*
                    complex:\s*.*
                \)
                .*
                CounterAgent\.get\(name:\s*string\)
            |
                CounterAgent\.get\(name:\s*string\)
                .*
                SampleAgent\.get\(
                    name:\s*string,\s*
                    region:\s*string,\s*
                    mode:\s*"fast"\s*\|\s*"safe",\s*
                    complex:\s*.*
                \)
            )
            .*"#
            };

            let methods_regex = indoc! {
                r#"(?sx)
            (
                sampleMethod:\s*\(\)\s*=>\s*number
                .*
                increment:\s*\(\)\s*=>\s*number
            |
                increment:\s*\(\)\s*=>\s*number
                .*
                sampleMethod:\s*\(\)\s*=>\s*number
            )"#
            };

            session.set_expect_timeout(Some(Duration::from_secs(300)));
            session.expect_str("golem-ts-repl[test-app-repl-interactive][local]>")?;
            session.send_line_and_expect_regex(".agent-type-info", agent_type_info_regex)?;
            session.expect_regex(methods_regex)?;

            session.set_expect_timeout(Some(Duration::from_secs(10)));

            // Hints on "enter"
            {
                session.send_line_wait_eval_expect_str("Counter", "CounterAgent")?;
                session.send_line_wait_eval_expect_regex("CounterAgent.", "get .* getPhantom ")?;
                session.send_line_wait_eval_expect_regex(
                    "CounterAgent.get",
                    "\\(method\\) CounterAgent\\.get\\(name: string\\): Promise<.*CounterAgent>",
                )?;
                session.send_line_wait_eval_expect_str("CounterAgent.get(", "\"?\"")?;
                session.send_line_wait_eval_expect_regex(
                    "CounterAgent.get(\"xyz\")",
                    "awaiting Promise<.*CounterAgent>",
                )?;

                session.send_line_wait_eval_expect_str(
                    "(await CounterAgent.get(\"xyz\")).",
                    "increment",
                )?;

                session.send_line_wait_eval_expect_str(
                    "CounterAgent.get(\"xyz\").then(c => c.",
                    "increment",
                )?;

                session.send_line_wait_eval_expect_str("Sample", "SampleAgent")?;

                session.send_line_wait_eval_expect_regex(
                    "(await SampleAgent.get(\"xyz\", \"eu\", \"fast\", { a: 1, b: \"x\" })).",
                    "sampleMethod",
                )?;

                session.send_line_wait_eval_expect_regex(
                    "SampleAgent.get(\"xyz\", \"eu\", \"fast\", { a: 1, b: \"x\" }).then(c => c.",
                    "sampleMethod",
                )?;
            }

            // Hints on "tab"
            {
                session.send_tab_wait_completion_expect_str("Counter", "Agent")?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "CounterAgent.",
                    "get .* getPhantom ",
                )?;
                session.kill_line()?;

                session.send_tab_wait_completion_expect_str("CounterAgent.g", "et")?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "CounterAgent.get",
                    "get .* getPhantom",
                )?;
                session.kill_line()?;

                session.send_tab_wait_completion_expect_str("CounterAgent.get(", "\"?\"")?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "(await CounterAgent.get(\"xyz\")).",
                    "increment",
                )?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "CounterAgent.get(\"xyz\").then(x => x.",
                    "increment",
                )?;
                session.kill_line()?;

                session.send_tab_wait_completion_expect_str("Sample", "Agent")?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "(await SampleAgent.get(\"xyz\", \"eu\", \"fast\", { a: 1, b: \"x\" })).",
                    "sampleMethod",
                )?;
                session.kill_line()?;

                session.send_tab_list_wait_completion_expect_regex(
                    "SampleAgent.get(\"xyz\", \"eu\", \"fast\", { a: 1, b: \"x\" }).then(x => x.",
                    "sampleMethod",
                )?;
                session.kill_line()?;
            }

            Ok(())
        },
    )
    .await;
}

#[test]
#[timeout("5 minutes")]
async fn ts_repl_foreground_cli_terminal_control(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-ts-repl-foreground-cli";

    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    ctx.cli_interactive_repl_test(
        [cmd::REPL, flag::LANGUAGE, "ts", "--disable-stream"],
        move |session| {
            session.set_expect_timeout(Some(Duration::from_secs(300)));
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session.set_expect_timeout(Some(Duration::from_secs(120)));
            session.send_line_and_expect_str(
                r#"await CounterAgent.get("x").then(_ => "AGENT_CREATED_1")"#,
                "AGENT_CREATED_1",
            )?;
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session.send_line_and_expect_regex(
                ".deploy --reset",
                r"Deleting [0-9]+ agents\(s\), do you want to continue",
            )?;
            session.send_line("y")?;
            session.expect_str("Reloading TypeScript REPL")?;
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session.send_line_and_expect_str(
                r#"await CounterAgent.get("x").then(_ => "AGENT_CREATED_2")"#,
                "AGENT_CREATED_2",
            )?;
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session.send_line_and_expect_regex(
                ".deploy --reset",
                r"Deleting [0-9]+ agents\(s\), do you want to continue",
            )?;
            session.send_ctrl_c()?;
            session.expect_str("Operation was interrupted by the user")?;
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session
                .send_line_and_expect_regex(".agent-list --refresh", r"CounterA|Agent|Component")?;
            std::thread::sleep(Duration::from_secs(2));
            session.send_ctrl_c()?;
            session.expect_regex(r"golem-ts-repl\[[^\]]+\]\[[^\]]+\]>")?;

            session.send_line(".exit")?;
            session.expect_eof()?;

            Ok(())
        },
    )
    .await;
}

#[test]
async fn basic_ifs_deploy(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-name

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              test-app-name:rust-main:
                templates: rust
                presets:
                  debug:
                    files:
                    - sourcePath: Cargo.toml
                      targetPath: /Cargo.toml
                      permissions: read-only
                    - sourcePath: src/lib.rs
                      targetPath: /src/lib.rs
                      permissions: read-write

        ", MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_ordered([
        "+components:",
        "+  test-app-name:rust-main:",
        "+    agents:",
        "+      CounterAgent:",
        "+        files:",
        "+          /Cargo.toml:",
        "+            permissions: readonly",
        "+            hash:",
        "+          /src/lib.rs:",
        "+            permissions: read-write",
        "+            hash:",
        "Planning",
        "- create component test-app-name:rust-main",
    ]));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-name

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              test-app-name:rust-main:
                templates: rust
                presets:
                  debug:
                    files:
                    - sourcePath: Cargo.toml
                      targetPath: /Cargo2.toml
                      permissions: read-only
                    - sourcePath: src/lib.rs
                      targetPath: /src/lib.rs
                      permissions: read-only

        ", MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_ordered([
        "  Planning ",
        "      Component changes:",
        "        - update component test-app-name:rust-main, changes:",
        "          - provision configs",
        "            - update agent type CounterAgent:",
        "              - files",
        "                - remove /Cargo.toml",
        "                - add /Cargo2.toml",
        "                - update /src/lib.rs (permissions)",
    ]));

    assert!(outputs.stdout_contains_ordered([
        "  Diffing ",
        "      @@ -8,13 +8,13 @@",
        "               mode: Durable",
        "               snapshotting:",
        "                 type: Disabled",
        "               files:",
        "      -          /Cargo.toml:",
        "      +          /Cargo2.toml:",
        "                   permissions: readonly",
        "                   hash:",
        "                 /src/lib.rs:",
        "      -            permissions: read-write",
        "      +            permissions: readonly",
        "                   hash:",
    ]));

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("Deployment: no changes required [UP-TO-DATE]"));
}

#[test]
async fn deploy_reset_allows_incompatible_config_and_secret_changes(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-reset-incompatible";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);
    ctx.start_server().await;

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("counter-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

            export const CounterAgent = defineAgent({
                name: 'CounterAgent',
                id: { name: z.string() },
                config: {
                    value: z.number(),
                },
                methods: {
                    increment: method({
                        input: {},
                        returns: z.number(),
                        promptHint: 'Increase the count by the configured amount',
                        description: 'Increases the count and returns the new value',
                    }),
                },
            });

            export const CounterAgentImpl = CounterAgent.implement({
                init: () => ({ count: 0 }),
                methods: {
                    increment() {
                        this.count += this.config.value;
                        return this.count;
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {
            r#"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-reset-incompatible

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-reset-incompatible:ts-main:
                templates: ts

            agents:
              CounterAgent:
                config:
                  value: 1
            "#,
            MANIFEST_VERSION = versions::sdk::MANIFEST
        },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("counter-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

            export const CounterAgent = defineAgent({
                name: 'CounterAgent',
                id: { name: z.string() },
                config: {
                    value: z.boolean(),
                },
                methods: {
                    increment: method({
                        input: {},
                        returns: z.number(),
                        promptHint: 'Increase the count based on the configured boolean',
                        description: 'Increases the count and returns the new value',
                    }),
                },
            });

            export const CounterAgentImpl = CounterAgent.implement({
                init: () => ({ count: 0 }),
                methods: {
                    increment() {
                        this.count += this.config.value ? 1 : 0;
                        return this.count;
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(!outputs.success_or_dump());
    assert!(outputs.stdout_contains("Old config value for agent CounterAgent at config key value is no longer valid due to an updated agent."));

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES, flag::RESET]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("Reset fallback"));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {
            r#"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-reset-incompatible

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-reset-incompatible:ts-main:
                templates: ts

            agents:
              CounterAgent:
                config:
                  value: true
            secretDefaults:
              local:
                secret: first
            "#,
            MANIFEST_VERSION = versions::sdk::MANIFEST
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("counter-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';

            export const CounterAgent = defineAgent({
                name: 'CounterAgent',
                id: { name: z.string() },
                config: {
                    value: z.boolean(),
                    secret: s.secret(z.string()),
                },
                methods: {
                    increment: method({
                        input: {},
                        returns: z.number(),
                        promptHint: 'Increase the count using the configured secret',
                        description: 'Increases the count and returns the new value',
                    }),
                },
            });

            export const CounterAgentImpl = CounterAgent.implement({
                init: () => ({ count: 0 }),
                methods: {
                    increment() {
                        this.count += this.config.value ? this.config.secret.get().length : 0;
                        return this.count;
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {
            r#"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-reset-incompatible

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-reset-incompatible:ts-main:
                templates: ts

            agents:
              CounterAgent:
                config:
                  value: true
            secretDefaults:
              local:
                secret: 42
            "#,
            MANIFEST_VERSION = versions::sdk::MANIFEST
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("counter-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';

            export const CounterAgent = defineAgent({
                name: 'CounterAgent',
                id: { name: z.string() },
                config: {
                    value: z.boolean(),
                    secret: s.secret(z.number()),
                },
                methods: {
                    increment: method({
                        input: {},
                        returns: z.number(),
                        promptHint: 'Increase the count using the configured numeric secret',
                        description: 'Increases the count and returns the new value',
                    }),
                },
            });

            export const CounterAgentImpl = CounterAgent.implement({
                init: () => ({ count: 0 }),
                methods: {
                    increment() {
                        this.count += this.config.value ? this.config.secret.get() : 0;
                        return this.count;
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(!outputs.success_or_dump());
    assert!(outputs.stdout_contains("AGENT_SECRET_NOT_COMPATIBLE: Agent secret at path secret is not compatible with existing secret in the environment."));

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES, flag::RESET]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("Reset fallback"));
}

#[test]
async fn component_level_ifs_with_multiple_agents_deploys(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-component-level-ifs";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("counter-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
            import { readFileSync } from 'node:fs';

            export const CounterAgent = defineAgent({
                name: 'CounterAgent',
                id: { name: z.string() },
                methods: {
                    increment: method({ input: {}, returns: z.number() }),
                    readFile: method({ input: { path: z.string() }, returns: z.string() }),
                },
            });

            export const CounterAgentImpl = CounterAgent.implement({
                init: () => ({}),
                methods: {
                    increment() {
                        return 1;
                    },
                    readFile({ path }) {
                        return readFileSync(path, 'utf8');
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("sample-agent.ts")),
        indoc! {
            r#"
            import { z } from 'zod';
            import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
            import { readFileSync } from 'node:fs';

            export const SampleAgent = defineAgent({
                name: 'SampleAgent',
                id: { name: z.string() },
                methods: {
                    sampleMethod: method({ input: {}, returns: z.number() }),
                    readFile: method({ input: { path: z.string() }, returns: z.string() }),
                },
            });

            export const SampleAgentImpl = SampleAgent.implement({
                init: () => ({}),
                methods: {
                    sampleMethod() {
                        return 1;
                    },
                    readFile({ path }) {
                        return readFileSync(path, 'utf8');
                    },
                },
            });
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("main.ts")),
        indoc! {
            r#"
            export const SHARED_IFS_MARKER = 'ifs-multi-agent-marker';
            import './counter-agent.js';
            import './sample-agent.js';
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-component-level-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-component-level-ifs:ts-main:
                templates: ts
                presets:
                  debug:
                    files:
                    - sourcePath: src/main.ts
                      targetPath: /main.ts
                      permissions: read-only
        ", MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stderr_contains("Found duplicated IFS targets"));
    assert!(outputs.stdout_contains("CounterAgent:"));
    assert!(outputs.stdout_contains("SampleAgent:"));
    assert!(outputs.stdout_contains("/main.ts:"));

    let counter_read_main = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "CounterAgent(\"counter-1\")",
            "readFile",
            "\"/main.ts\"",
        ])
        .await;
    assert!(counter_read_main.success_or_dump());
    assert!(counter_read_main.stdout_contains("SHARED_IFS_MARKER"));
    assert!(counter_read_main.stdout_contains("ifs-multi-agent-marker"));

    let sample_read_main = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "SampleAgent(\"sample-1\")",
            "readFile",
            "\"/main.ts\"",
        ])
        .await;
    assert!(sample_read_main.success_or_dump());
    assert!(sample_read_main.stdout_contains("SHARED_IFS_MARKER"));
    assert!(sample_read_main.stdout_contains("ifs-multi-agent-marker"));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-component-level-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-component-level-ifs:ts-main:
                templates: ts

            agents:
              CounterAgent:
                files:
                - sourcePath: src/main.ts
                  targetPath: /counter-main.ts
                  permissions: read-only
              SampleAgent:
                files:
                - sourcePath: src/main.ts
                  targetPath: /sample-main.ts
                  permissions: read-only
        ", MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stderr_contains("Found duplicated IFS targets"));
    assert!(outputs.stdout_contains("/counter-main.ts:"));
    assert!(outputs.stdout_contains("/sample-main.ts:"));

    let counter_read_agent_target = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "CounterAgent(\"counter-2\")",
            "readFile",
            "\"/counter-main.ts\"",
        ])
        .await;
    assert!(counter_read_agent_target.success_or_dump());
    assert!(counter_read_agent_target.stdout_contains("SHARED_IFS_MARKER"));
    assert!(counter_read_agent_target.stdout_contains("ifs-multi-agent-marker"));

    let sample_read_agent_target = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "SampleAgent(\"sample-2\")",
            "readFile",
            "\"/sample-main.ts\"",
        ])
        .await;
    assert!(sample_read_agent_target.success_or_dump());
    assert!(sample_read_agent_target.stdout_contains("SHARED_IFS_MARKER"));
    assert!(sample_read_agent_target.stdout_contains("ifs-multi-agent-marker"));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {"
            manifestVersion: {MANIFEST_VERSION}

            app: test-app-component-level-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-component-level-ifs:ts-main:
                templates: ts

            agents:
              CounterAgent:
                files:
                - sourcePath: src/main.ts
                  targetPath: /shared.ts
                  permissions: read-only
              SampleAgent:
                files:
                - sourcePath: src/sample-agent.ts
                  targetPath: /shared.ts
                  permissions: read-only
        ", MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stderr_contains("Found duplicated IFS targets"));
    assert!(outputs.stdout_contains("/shared.ts:"));

    let counter_read_shared = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "CounterAgent(\"counter-3\")",
            "readFile",
            "\"/shared.ts\"",
        ])
        .await;
    assert!(counter_read_shared.success_or_dump());
    assert!(counter_read_shared.stdout_contains("SHARED_IFS_MARKER"));

    let sample_read_shared = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "SampleAgent(\"sample-3\")",
            "readFile",
            "\"/shared.ts\"",
        ])
        .await;
    assert!(sample_read_shared.success_or_dump());
    assert!(sample_read_shared.stdout_contains("class SampleAgent"));
}

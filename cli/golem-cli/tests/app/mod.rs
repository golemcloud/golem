// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

mod plugins;

use crate::test_r_get_dep_tracing;
use crate::Tracing;
use assert2::{assert, check, let_assert};
use colored::Colorize;
use golem_cli::fs;
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_templates::model::GuestLanguage;
use heck::ToKebabCase;
use indoc::indoc;
use itertools::Itertools;
use nanoid::nanoid;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use strum::IntoEnumIterator;
use tempfile::TempDir;
use test_r::test;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

mod cmd {
    pub static ADD_DEPENDENCY: &str = "add-dependency";
    pub static AGENT: &str = "agent";
    pub static APP: &str = "app";
    pub static BUILD: &str = "build";
    pub static COMPLETION: &str = "completion";
    pub static COMPONENT: &str = "component";
    pub static DEPLOY: &str = "deploy";
    pub static GET: &str = "get";
    pub static INVOKE: &str = "invoke";
    pub static NEW: &str = "new";
    pub static PLUGIN: &str = "plugin";
    pub static REGISTER: &str = "register";
    pub static REPL: &str = "repl";
    pub static TEMPLATES: &str = "templates";
}

mod flag {
    pub static DEV_MODE: &str = "--dev-mode";
    pub static FORCE_BUILD: &str = "--force-build";
    pub static REDEPLOY_ALL: &str = "--redeploy-all";
    pub static SCRIPT: &str = "--script";
    pub static FORMAT: &str = "--format";
    pub static SHOW_SENSITIVE: &str = "--show-sensitive";
    pub static YES: &str = "--yes";
}

mod pattern {
    pub static ERROR: &str = "error";
    pub static HELP_APPLICATION_COMPONENTS: &str = "Application components:";
    pub static HELP_APPLICATION_CUSTOM_COMMANDS: &str = "Application custom commands:";
    pub static HELP_COMMANDS: &str = "Commands:";
    pub static HELP_USAGE: &str = "Usage:";
}

#[test]
async fn app_help_in_empty_folder(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
    check!(outputs.stderr_contains(pattern::HELP_COMMANDS));
    check!(!outputs.stderr_contains(pattern::ERROR));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
}

#[test]
async fn app_new_with_many_components_and_then_help_in_app_folder(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx
        .cli([cmd::APP, cmd::NEW, app_name, "typescript", "rust"])
        .await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "typescript", "app:typescript"])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
    check!(outputs.stderr_contains(pattern::HELP_COMMANDS));
    check!(!outputs.stderr_contains(pattern::ERROR));
    check!(outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    check!(outputs.stderr_contains("app:rust"));
    check!(outputs.stderr_contains("app:typescript"));
    check!(outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
    check!(outputs.stderr_contains("cargo-clean"));
    check!(outputs.stderr_contains("npm-install"));
}

#[test]
async fn app_build_with_rust_component(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success());

    // First build
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stdout_contains("Compiling app_rust v0.0.1"));

    check_component_metadata(
        &ctx.working_dir
            .join("golem-temp/components/app_rust_debug.wasm"),
        "app:rust".to_string(),
        None,
    );

    // Rebuild - 1
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 2
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 3 - force, but cargo is smart to skip actual compile
    let outputs = ctx.cli([cmd::APP, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stdout_contains("Finished `dev` profile"));

    // Rebuild - 4
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Clean
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    // Rebuild - 5
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));
}

#[test]
async fn app_new_language_hints(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP, cmd::NEW, "dummy-app-name"]).await;
    assert!(!outputs.success());
    check!(outputs.stdout_contains("Available languages:"));

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
async fn basic_dependencies_build(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-other"])
        .await;
    assert!(outputs.success());

    fs::append_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust")
                .join("golem.yaml"),
        ),
        indoc! {"
            dependencies:
              app:rust:
              - target: app:rust
                type: wasm-rpc
              - target: app:rust-other
                type: wasm-rpc
        "},
    )
    .unwrap();

    fs::append_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust-other")
                .join("golem.yaml"),
        ),
        indoc! {"
            dependencies:
              app:rust-other:
              - target: app:rust
                type: wasm-rpc
              - target: app:rust-other
                type: wasm-rpc
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    check!(outputs.stderr_count_lines_containing("- app:rust (wasm-rpc)") == 2);
    check!(outputs.stderr_count_lines_containing("- app:rust-other (wasm-rpc)") == 2);

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
}

#[test]
async fn basic_ifs_deploy(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success());

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              app:rust:
                template: rust
                profiles:
                  debug:
                    files:
                    - sourcePath: Cargo.toml
                      targetPath: /Cargo.toml
                      permissions: read-only
                    - sourcePath: src/lib.rs
                      targetPath: /src/lib.rs
                      permissions: read-write

        "},
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());
    check!(outputs.stdout_contains("Creating component app:rust"));
    check!(outputs.stdout_contains("ro /Cargo.toml"));
    check!(outputs.stdout_contains("rw /src/lib.rs"));

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              app:rust:
                template: rust
                profiles:
                  debug:
                    files:
                    - sourcePath: Cargo.toml
                      targetPath: /Cargo2.toml
                      permissions: read-only
                    - sourcePath: src/lib.rs
                      targetPath: /src/lib.rs
                      permissions: read-only

        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());
    check!(outputs.stdout_contains("Updating component app:rust"));
    check!(!outputs.stdout_contains("ro /Cargo.toml"));
    check!(outputs.stdout_contains("ro /Cargo2.toml"));
    check!(!outputs.stdout_contains("rw /src/lib.rs"));
    check!(outputs.stdout_contains("ro /src/lib.rs"));

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains("Skipping deploying component app:rust"));
}

#[test]
async fn custom_app_subcommand_with_builtin_name() {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success());

    fs::append_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"
            customCommands:
              new:
                - command: cargo tree
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(":new"));

    let outputs = ctx.cli([cmd::APP, ":new"]).await;
    assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo tree'"));
}

#[test]
async fn wasm_library_dependency_type() -> anyhow::Result<()> {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:main"])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "app:lib"]).await;
    assert!(outputs.success());

    // Changing the `app:lib` component type to be a library
    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-lib")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              app:lib:
                template: rust
                profiles:
                  debug:
                    componentType: library
                  release:
                    componentType: library
        "},
    )?;

    // Adding as a wasm dependency

    fs::append_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-main")
                .join("golem.yaml"),
        ),
        indoc! {"
            dependencies:
              app:main:
                - type: wasm
                  target: app:lib
        "},
    )?;

    // Rewriting the main WIT

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-main")
                .join("wit")
                .join("app-main.wit"),
        ),
        indoc! {"
            package app:main;

            interface app-main-api {
                run: func() -> u64;
            }

            world app-main {
                export app-main-api;
                import app:lib-exports/app-lib-api;
            }
        "},
    )?;

    // Rewriting the main Rust source code

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-main")
                .join("src")
                .join("lib.rs"),
        ),
        indoc! {"
                #[allow(static_mut_refs)]
                mod bindings;

                use bindings::app::lib_exports::app_lib_api;
                use crate::bindings::exports::app::main_exports::app_main_api::*;

                struct Component;

                impl Guest for Component {
                    fn run() -> u64 {
                        app_lib_api::add(1);
                        app_lib_api::add(2);
                        app_lib_api::add(3);
                        app_lib_api::get()
                    }
                }

                bindings::export!(Component with_types_in bindings);
         "},
    )?;

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            "app:main/test1",
            "run",
            "--format",
            "json",
        ])
        .await;
    assert!(outputs.success());

    let result: InvokeResultView = serde_json::from_str(&outputs.stdout[0])?;
    assert_eq!(result.result_wave, Some(vec!["6".to_string()]));

    Ok(())
}

#[test]
async fn adding_and_changing_rpc_deps_retriggers_build() {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    // Setup app
    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-a"])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-b"])
        .await;
    assert!(outputs.success());

    // Build app
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    // Add wasm-rpc dependencies
    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-b",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-b",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains_ordered([
        "Application components:",
        "  app:rust-a",
        "    Dependencies:",
        "      - app:rust-a (wasm-rpc)",
        "      - app:rust-b (wasm-rpc)",
        "  app:rust-b",
        "    Dependencies:",
        "      - app:rust-a (wasm-rpc)",
    ]));

    // Build with dynamic deps
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Copying app:rust-a without linking, no static dependencies were found",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Copying app:rust-b without linking, no static dependencies were found",
    ]));

    // Build again with dynamic deps, now it should skip
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Skipping linking dependencies for app:rust-a, UP-TO-DATE",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Skipping linking dependencies for app:rust-b, UP-TO-DATE",
    ]));

    // Update deps to static
    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-b",
            "--dependency-type",
            "static-wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-b",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "static-wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "static-wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains_ordered([
        "Application components:",
        "  app:rust-a",
        "    Dependencies:",
        "      - app:rust-a (static-wasm-rpc)",
        "      - app:rust-b (static-wasm-rpc)",
        "  app:rust-b",
        "    Dependencies:",
        "      - app:rust-a (static-wasm-rpc)",
    ]));

    // Build with static deps
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found static WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Linking static dependencies (app:rust-a, app:rust-b) into app:rust-a",
        "  Found static WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Linking static dependencies (app:rust-a) into app:rust-b",
    ]));

    // Build with static deps again, should skip
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found static WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Skipping linking dependencies for app:rust-a, UP-TO-DATE",
        "  Found static WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Skipping linking dependencies for app:rust-b, UP-TO-DATE",
    ]));

    // Switching back to dynamic deps
    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-b",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-b",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::COMPONENT,
            cmd::ADD_DEPENDENCY,
            "--component-name",
            "app:rust-a",
            "--target-component-name",
            "app:rust-a",
            "--dependency-type",
            "wasm-rpc",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains_ordered([
        "Application components:",
        "  app:rust-a",
        "    Dependencies:",
        "      - app:rust-a (wasm-rpc)",
        "      - app:rust-b (wasm-rpc)",
        "  app:rust-b",
        "    Dependencies:",
        "      - app:rust-a (wasm-rpc)",
    ]));

    // Build with dynamic deps, should not skip
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Copying app:rust-a without linking, no static dependencies were found",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Copying app:rust-b without linking, no static dependencies were found",
    ]));

    // Build again with dynamic deps, now it should skip again
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Skipping linking dependencies for app:rust-a, UP-TO-DATE",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Skipping linking dependencies for app:rust-b, UP-TO-DATE",
    ]));
}

#[test]
async fn build_and_deploy_all_templates() {
    let mut ctx = TestContext::new();
    let app_name = "all-templates-app";

    let outputs = ctx.cli([cmd::COMPONENT, cmd::TEMPLATES]).await;
    assert!(outputs.success());

    let template_prefix = "  - ";

    let templates = outputs
        .stdout
        .iter()
        .filter(|line| line.starts_with(template_prefix) && line.contains(':'))
        .map(|line| {
            let template_with_desc = line.as_str().strip_prefix(template_prefix);
            let_assert!(Some(template_with_desc) = template_with_desc, "{}", line);
            let separator_index = template_with_desc.find(":");
            let_assert!(
                Some(separator_index) = separator_index,
                "{}",
                template_with_desc
            );

            &template_with_desc[..separator_index]
        })
        .collect::<Vec<_>>();

    println!("{templates:#?}");

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let alphabet: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
    for template in templates {
        for _ in 0..2 {
            let component_name = format!(
                "app:{}-{}",
                template.to_kebab_case(),
                nanoid!(10, &alphabet),
            );
            let outputs = ctx
                .cli([cmd::COMPONENT, cmd::NEW, template, &component_name])
                .await;
            assert!(outputs.success());
        }
    }

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success());
}

#[test]
async fn test_ts_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter"])
        .await;
    assert!(outputs.success());

    let uuid = Uuid::new_v4().to_string();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("app:counter/counter-agent(\"{uuid}\")"),
            "increment",
        ])
        .await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains("- 1"));
}

// Invocations on code-first typescript agents, with complex types / functions.
// Each function call is executed via RPC, and at every stage, mostly return type is same as input type.
// Early in the code-first release, some of these cases failed at the Golem execution stage
// (post type extraction). This test ensures such issues are caught automatically
// and act as a regression-test.
#[test]
async fn test_ts_code_first_complex() {
    let mut ctx = TestContext::new();

    let app_name = "ts-code-first";

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;

    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "ts:agent"]).await;

    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("golem.yaml"),
    );

    let component_source_code_main_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("main.ts"),
    );

    let component_source_code_model_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("model.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              ts:agent:
                template: ts
        "# },
    )
    .unwrap();

    fs::copy(
        "test-data/ts-code-first-snippets/main.ts",
        &component_source_code_main_file,
    )
    .unwrap();

    fs::copy(
        "test-data/ts-code-first-snippets/model.ts",
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("ts:agent/foo-agent(\"{uuid}\")");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success(), "function {func} failed");
    }

    // fun with void return
    run_and_assert(&ctx, "fun-void-return", &["\"sample\""]).await;

    // fun without return type
    run_and_assert(&ctx, "fun-no-return", &["\"sample\""]).await;

    // function optional (that has null, defined as union)
    run_and_assert(
        &ctx,
        "fun-optional",
        &[
            "some(case1(\"foo\"))",
            "{a: some(\"foo\")}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(\"foo\")}",
            "some(\"foo\")",
            "some(case3(\"foo\"))",
        ],
    )
    .await;

    // function with a simple object
    run_and_assert(&ctx, "fun-object-type", &[r#"{a: "foo", b: 42, c: true}"#]).await;

    // function with a very complex object
    let argument = r#"
      {a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}
    "#;

    run_and_assert(&ctx, "fun-object-complex-type", &[argument]).await;

    // union type that has anonymous terms
    run_and_assert(&ctx, "fun-union-type", &[r#"union-type1("foo")"#]).await;

    // A complex union type
    run_and_assert(
        &ctx,
        "fun-union-complex-type",
        &[r#"union-complex-type1("foo")"#],
    )
    .await;

    // Union that includes literals and boolean (string literal input)
    run_and_assert(&ctx, "fun-union-with-literals", &[r#"lit1"#]).await;

    // Union that includes literals and boolean (boolean input)
    run_and_assert(
        &ctx,
        "fun-union-with-literals",
        &[r#"union-with-literals1(true)"#],
    )
    .await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["foo"]).await;

    // Unstructured text type
    run_and_assert(&ctx, "fun-unstructured-text", &["url(\"foo\")"]).await;

    // Unstructured binary
    run_and_assert(&ctx, "fun-unstructured-binary", &["url(\"foo\")"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["bar"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["baz"]).await;

    // A number type
    run_and_assert(&ctx, "fun-number", &["42"]).await;

    // A string type
    run_and_assert(&ctx, "fun-string", &[r#""foo""#]).await;

    // A boolean type
    run_and_assert(&ctx, "fun-boolean", &["true"]).await;

    // A map type
    run_and_assert(&ctx, "fun-map", &[r#"[("foo", 42), ("bar", 42)]"#]).await;

    assert!(outputs.success());

    // A tagged union
    run_and_assert(&ctx, "fun-tagged-union", &[r#"a("foo")"#]).await;

    assert!(outputs.success());

    // A simple tuple type
    run_and_assert(&ctx, "fun-tuple-type", &[r#"("foo", 42, true)"#]).await;

    // A complex tuple type
    run_and_assert(
        &ctx,
        "fun-tuple-complex-type",
        &[r#"("foo", 42, {a: "foo", b: 42, c: true})"#],
    )
    .await;

    // A list complex type
    run_and_assert(
        &ctx,
        "fun-list-complex-type",
        &[r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#],
    ).await;

    // A function with null return
    run_and_assert(&ctx, "fun-null-return", &[r#""foo""#]).await;

    // A function with undefined return
    run_and_assert(&ctx, "fun-undefined-return", &[r#""foo""#]).await;

    // A function with result type
    run_and_assert(&ctx, "fun-result-exact", &[r#"ok("foo")"#]).await;

    // A function with (untagged) result-like type - but not result
    run_and_assert(
        &ctx,
        "fun-either-optional",
        &[r#"{ok: some("foo"), err: none}"#],
    )
    .await;

    // An arrow function
    run_and_assert(&ctx, "fun-arrow-sync", &[r#""foo""#]).await;

    // A function that takes many inputs
    run_and_assert(
            &ctx,
            "fun-all",
            &[
                r#"{a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}"#,
                r#"union-type1("foo")"#,
                r#"union-complex-type1("foo")"#,
                r#"42"#,
                r#""foo""#,
                r#"true"#,
                r#"[("foo", 42), ("foo", 42), ("foo", 42)]"#,
                r#"("foo", 42, {a: "foo", b: 42, c: true})"#,
                r#"("foo", 42, true)"#,
                r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#,
                r#"{a: "foo", b: 42, c: true}"#,
                r#"okay("foo")"#,
                r#"{ok: some("foo"), err: some("foo")}"#,
                r#"some(case1("foo"))"#,
                r#"{a: some("foo")}"#,
                r#"{a: some(case1("foo"))}"#,
                r#"{a: some(case1("foo"))}"#,
                r#"{a: some("foo")}"#,
                r#"some("foo")"#,
                r#"some(case3("foo"))"#,
                r#"a("foo")"#
            ],
        )
        .await;
}

#[test]
async fn test_common_dep_plugs_errors() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );
    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts

            dependencies:
              app:weather-agent:
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_brave-dev.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_google-dev.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_serper-dev.wasm
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_tavily-dev.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains_ordered(
        [
            "error: an error occurred when building the composition graph: multiple plugs found for export golem:web-search/types@1.0.0, only use one of them:",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_brave-dev.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_google-dev.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_serper-dev.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_tavily-dev.wasm",
        ]
    ));

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts

            dependencies:
              app:weather-agent:
              - type: wasm
                url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0-dev.6/golem_web_search_brave-dev.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts
        "# },
    )
    .unwrap();

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import * as websearch from 'golem:web-search/web-search@1.0.0';

            @agent()
            class Agent extends BaseAgent {
              async search(query: string): Promise<string> {
                let result = websearch.searchOnce({
                  query: query,
                });

                console.log(result);

                return "ok";
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "agent()",
            "search",
            "query",
        ])
        .await;
    assert!(!outputs.success());
    // If this fails, then adjust format_stack_line to match it
    assert!(outputs.stderr_contains("Library golem:web-search/web-search@1.0.0 called without being linked with an implementation"))
}

#[test]
async fn test_component_env_var_substitution() {
    let mut ctx = TestContext::new();
    let app_name = "env_var_substitution";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                template: ts
                env:
                  NORMAL: 'REALLY'
                  VERY_CUSTOM_ENV_VAR_SECRET_1: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}'
                  VERY_CUSTOM_ENV_VAR_SECRET_2: '{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
                  COMPOSED: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
        "# },
    )
    .unwrap();

    ctx.start_server();

    // Building is okay, as that does not resolve env vars
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    // But deploying will do so, so it should fail
    let outputs = ctx.cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY]).await;
    assert!(!outputs.success());

    assert!(outputs.stderr_contains_ordered([
        "key:       COMPOSED",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_1",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_2",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "Failed to prepare environment variables for component: app:weather-agent",
    ]));

    // After providing the missing env vars, deploy should work
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_1", "123");
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_3", "456");

    let outputs = ctx.cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains_ordered([
        "COMPOSED=123-456",
        "NORMAL=REALLY",
        "VERY_CUSTOM_ENV_VAR_SECRET_1=123",
        "VERY_CUSTOM_ENV_VAR_SECRET_2=456",
    ]));
}

#[test]
async fn test_http_api_merging() {
    let mut ctx = TestContext::new();
    let app_name = "http_api_merging";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter1"])
        .await;
    assert!(outputs.success());
    let component1_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter1")
            .join("golem.yaml"),
    );

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter2"])
        .await;
    assert!(outputs.success());
    let component2_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter2")
            .join("golem.yaml"),
    );

    // Add mergeable definitions and deployments to both components
    fs::write_str(
        &component1_manifest_path,
        indoc! { r#"
            components:
              app:counter1:
                template: ts
            
            httpApi:
              definitions:
                def-a:
                  version: 0.0.1
                  routes:
                  - method: GET
                    path: /a
                    binding:
                      componentName: app:counter1
                      response: |
                        let agent = counter-agent("a");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-a
        "# },
    )
    .unwrap();

    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    // Check that the merged manifest is loadable
    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(!outputs.stdout_contains("error"));
    assert!(!outputs.stderr_contains("error"));
    assert!(outputs.stderr_contains_ordered([
        "Application API definitions:",
        "  def-a@0.0.1",
        "  def-b@0.0.2",
        "Application API deployments for profile local:",
        "  localhost:9006",
        "    def-a",
        "    def-b",
    ]));

    // But we still cannot define the same deployment <-> definition in two places:
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
                  - def-a
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(
        "error: HTTP API Deployment local - localhost:9006 - def-a is defined in multiple sources"
    ));

    // Let's switch back to the good config and deploy, then call the exposed APIs
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                template: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - host: localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx
        .cli([cmd::APP, cmd::DEPLOY, flag::REDEPLOY_ALL, flag::YES])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "API def-a/0.0.1 deployed at localhost:9006",
        "API def-b/0.0.2 deployed at localhost:9006"
    ]));
}

#[test]
async fn test_invoke_and_repl_agent_id_casing_and_normalizing() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "app:agent"]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent, } from '@golemcloud/golem-ts-sdk';

            type Complex = {
              oneField: string;
              anotherField: number;
            }

            @agent()
            class LongAgentName extends BaseAgent {
              params: Complex;
              constructor(params: Complex) {
                super();
                this.params = params;
              }

              async ask(question: Complex): Promise<[Complex, Complex]> {
                return [this.params, question];
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server();

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            flag::REDEPLOY_ALL,
            r#"long-agent-name({one-field: "1212", another-field: 100})"#,
            "ask",
            r#"{one-field: "1", another-field: 2}"#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        r#"long-agent-name({one-field:"1212",another-field:100})"#,
        r#"({one-field: "1212", another-field: 100}, {one-field: "1", another-field: 2})"#,
    ]));

    let outputs = ctx
        .cli([
            cmd::REPL,
            flag::FORMAT,
            "json",
            flag::SCRIPT,
            r#"
                let x = long-agent-name({one-field: "1212", another-field: 100});
                x.ask({one-field: "1", another-field: 2}) 
            "#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains(
        r#"{"another-field":100.0,"one-field":"1212"},{"another-field":2.0,"one-field":"1"}"#
    ));
}

enum CommandOutput {
    Stdout(String),
    Stderr(String),
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

impl Output {
    pub async fn stream_and_collect(prefix: &str, child: &mut Child) -> io::Result<Self> {
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stdout"));

        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stderr"));

        let (tx, mut rx) = mpsc::unbounded_channel();

        tokio::spawn({
            let prefix = format!("> {} - stdout:", prefix).green().bold();
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("{prefix} {line}");
                    tx.send(CommandOutput::Stdout(line)).unwrap();
                }
            }
        });

        tokio::spawn({
            let prefix = format!("> {} - stderr:", prefix).red().bold();
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("{prefix} {line}");
                    tx.send(CommandOutput::Stderr(line)).unwrap();
                }
            }
        });

        drop(tx);

        let mut stdout = vec![];
        let mut stderr = vec![];
        while let Some(item) = rx.recv().await {
            match item {
                CommandOutput::Stdout(line) => stdout.push(line),
                CommandOutput::Stderr(line) => stderr.push(line),
            }
        }

        Ok(Self {
            status: child.wait().await?,
            stdout,
            stderr,
        })
    }

    #[must_use]
    fn success(&self) -> bool {
        self.status.success()
    }

    #[must_use]
    fn stdout_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stdout
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    fn stdout_contains_row_with_cells(&self, expected_cells: &[&str]) -> bool {
        self.stdout
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| {
                let cells = line.split('|').map(str::trim).collect::<HashSet<_>>();
                expected_cells.iter().all(|cell| cells.contains(cell))
            })
    }

    #[must_use]
    fn stderr_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stderr
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    #[must_use]
    fn stdout_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(&self.stdout, patterns)
    }

    #[must_use]
    fn stderr_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(&self.stderr, patterns)
    }

    #[allow(dead_code)]
    #[must_use]
    fn stdout_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stdout
            .iter()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }

    #[must_use]
    fn stderr_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stderr
            .iter()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }
}

impl From<std::process::Output> for Output {
    fn from(output: std::process::Output) -> Self {
        fn to_lines(bytes: Vec<u8>) -> Vec<String> {
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|s| s.to_string())
                .collect()
        }

        Self {
            status: output.status,
            stdout: to_lines(output.stdout),
            stderr: to_lines(output.stderr),
        }
    }
}

#[derive(Debug)]
struct TestContext {
    golem_path: PathBuf,
    golem_cli_path: PathBuf,
    _test_dir: TempDir,
    config_dir: TempDir,
    data_dir: TempDir,
    working_dir: PathBuf,
    server_process: Option<Child>,
    env: HashMap<String, String>,
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let server_process = self.server_process.take();
        tokio::spawn(async move {
            if let Some(mut server_process) = server_process {
                println!("{}", "> stopping golem server".bold());
                server_process.kill().await.unwrap();
            }
        });
    }
}

impl TestContext {
    fn new() -> Self {
        let test_dir = TempDir::new().unwrap();
        let working_dir = test_dir.path().to_path_buf();

        let ctx = Self {
            golem_path: PathBuf::from("../../target/debug/golem")
                .canonicalize()
                .unwrap_or_else(|_| {
                    panic!(
                        "golem binary not found in ../../target/debug/golem, with current dir: {:?}",
                        std::env::current_dir().unwrap()
                    );
                }),
            golem_cli_path: PathBuf::from("../../target/debug/golem-cli")
                .canonicalize()
                .unwrap_or_else(|_| {
                    panic!(
                        "golem binary not found in ../../target/debug/golem-cli, with current dir: {:?}",
                        std::env::current_dir().unwrap()
                    );
                }),
            _test_dir: test_dir,
            config_dir: TempDir::new().unwrap(),
            data_dir: TempDir::new().unwrap(),
            working_dir,
            server_process: None,
            env: HashMap::new(),
        };

        info!(ctx = ?ctx ,"Created test context");

        ctx
    }

    #[allow(dead_code)]
    fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    fn env_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.env
    }

    fn add_env_var<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.env_mut().insert(key.into(), value.into());
    }

    #[must_use]
    async fn cli<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = {
            let mut all_args = vec![
                "--config-dir".to_string(),
                self.config_dir.path().to_str().unwrap().to_string(),
                flag::DEV_MODE.to_string(),
            ];
            all_args.extend(
                args.into_iter()
                    .map(|a| a.as_ref().to_str().unwrap().to_string()),
            );
            all_args
        };
        let working_dir = &self.working_dir.canonicalize().unwrap();

        println!(
            "{} {}",
            "> working directory:".bold(),
            working_dir.display()
        );
        println!("{} {}", "> golem-cli".bold(), args.iter().join(" ").blue());

        let mut child = Command::new(&self.golem_cli_path)
            .args(args)
            .envs(&self.env)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        Output::stream_and_collect("golem-cli", &mut child)
            .await
            .unwrap()
    }

    fn start_server(&mut self) {
        assert!(self.server_process.is_none(), "server is already running");

        println!("{}", "> starting golem server".bold());
        println!(
            "{} {}",
            "> server config directory:".bold(),
            self.config_dir.path().display()
        );
        println!(
            "{} {}",
            "> server data directory:".bold(),
            self.data_dir.path().display()
        );

        self.server_process = Some(
            Command::new(&self.golem_path)
                .args([
                    "server",
                    "run",
                    "--config-dir",
                    self.config_dir.path().to_str().unwrap(),
                    "--data-dir",
                    self.data_dir.path().to_str().unwrap(),
                ])
                .current_dir(&self.working_dir)
                .spawn()
                .unwrap(),
        )
    }

    fn cd<P: AsRef<Path>>(&mut self, path: P) {
        self.working_dir = self.working_dir.join(path.as_ref());
    }

    fn cwd_path_join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.working_dir.join(path)
    }
}

fn check_component_metadata(
    wasm: &Path,
    expected_package_name: String,
    expected_version: Option<String>,
) {
    let wasm = std::fs::read(wasm).unwrap();
    let payload = wasm_metadata::Payload::from_binary(&wasm).unwrap();
    let metadata = payload.metadata();

    assert_eq!(metadata.name, Some(expected_package_name));
    assert_eq!(
        metadata.version.as_ref().map(|v| v.to_string()),
        expected_version
    );
}

#[must_use]
fn contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
    lines: &[String],
    patterns: I,
) -> bool {
    let mut patterns = patterns.into_iter();
    let mut pattern = patterns.next();
    let mut pattern_str = pattern.as_ref().map(|s| s.as_ref());
    for line in lines {
        match pattern_str {
            Some(p) => {
                if line.contains(p) {
                    pattern = patterns.next();
                    pattern_str = pattern.as_ref().map(|s| s.as_ref());
                }
            }
            None => {
                break;
            }
        }
    }
    let remaining_patterns = pattern_str
        .into_iter()
        .map(|s| s.to_string())
        .chain(patterns.map(|s| s.as_ref().to_string()))
        .collect::<Vec<_>>();
    if !remaining_patterns.is_empty() {
        println!("{}", "Missing patterns:".red().underline());
        for pattern in &remaining_patterns {
            println!("{pattern}");
        }
    }
    remaining_patterns.is_empty()
}

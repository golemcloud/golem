use crate::app::{check_component_metadata, cmd, flag, pattern, TestContext};
use crate::Tracing;
use assert2::check;
use golem_cli::fs;
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_templates::model::GuestLanguage;
use indoc::indoc;
use std::path::Path;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
async fn app_help_in_empty_folder(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP]).await;
    assert2::assert!(!outputs.success());
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
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "typescript", "app:typescript"])
        .await;
    assert2::assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert2::assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert2::assert!(!outputs.success());
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
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert2::assert!(outputs.success());

    // First build
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
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
    assert2::assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 2
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 3 - force, but cargo is smart to skip actual compile
    let outputs = ctx.cli([cmd::APP, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert2::assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stdout_contains("Finished `dev` profile"));

    // Rebuild - 4
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Clean
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());

    // Rebuild - 5
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));
}

#[test]
async fn app_new_language_hints(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP, cmd::NEW, "dummy-app-name"]).await;
    assert2::assert!(!outputs.success());
    check!(outputs.stdout_contains("Available languages:"));

    let languages_without_templates = GuestLanguage::iter()
        .filter(|language| !outputs.stdout_contains(format!("- {language}")))
        .collect::<Vec<_>>();

    assert2::assert!(
        languages_without_templates.is_empty(),
        "{:?}",
        languages_without_templates
    );
}

#[test]
async fn completion(_tracing: &Tracing) {
    let ctx = TestContext::new();

    let outputs = ctx.cli([cmd::COMPLETION, "bash"]).await;
    assert2::assert!(outputs.success(), "bash");

    let outputs = ctx.cli([cmd::COMPLETION, "elvish"]).await;
    assert2::assert!(outputs.success(), "elvish");

    let outputs = ctx.cli([cmd::COMPLETION, "fish"]).await;
    assert2::assert!(outputs.success(), "fish");

    let outputs = ctx.cli([cmd::COMPLETION, "powershell"]).await;
    assert2::assert!(outputs.success(), "powershell");

    let outputs = ctx.cli([cmd::COMPLETION, "zsh"]).await;
    assert2::assert!(outputs.success(), "zsh");
}

#[test]
async fn basic_dependencies_build(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert2::assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-other"])
        .await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(!outputs.success());
    check!(outputs.stderr_count_lines_containing("- app:rust (wasm-rpc)") == 2);
    check!(outputs.stderr_count_lines_containing("- app:rust-other (wasm-rpc)") == 2);

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
}

#[test]
async fn basic_ifs_deploy(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());
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
    assert2::assert!(outputs.success());
    check!(outputs.stdout_contains("Updating component app:rust"));
    check!(!outputs.stdout_contains("ro /Cargo.toml"));
    check!(outputs.stdout_contains("ro /Cargo2.toml"));
    check!(!outputs.stdout_contains("rw /src/lib.rs"));
    check!(outputs.stdout_contains("ro /src/lib.rs"));

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains("Skipping deploying component app:rust"));
}

#[test]
async fn custom_app_subcommand_with_builtin_name() {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(!outputs.success());
    check!(outputs.stderr_contains(":new"));

    let outputs = ctx.cli([cmd::APP, ":new"]).await;
    assert2::assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo tree'"));
}

#[test]
async fn wasm_library_dependency_type() -> anyhow::Result<()> {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:main"])
        .await;
    assert2::assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "app:lib"]).await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-a"])
        .await;
    assert2::assert!(outputs.success());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust-b"])
        .await;
    assert2::assert!(outputs.success());

    // Build app
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert2::assert!(!outputs.success());
    assert2::assert!(outputs.stderr_contains_ordered([
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
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Copying app:rust-a without linking, no static dependencies were found",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Copying app:rust-b without linking, no static dependencies were found",
    ]));

    // Build again with dynamic deps, now it should skip
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert2::assert!(!outputs.success());
    assert2::assert!(outputs.stderr_contains_ordered([
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
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found static WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Linking static dependencies (app:rust-a, app:rust-b) into app:rust-a",
        "  Found static WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Linking static dependencies (app:rust-a) into app:rust-b",
    ]));

    // Build with static deps again, should skip
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]).await;
    assert2::assert!(!outputs.success());
    assert2::assert!(outputs.stderr_contains_ordered([
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
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Copying app:rust-a without linking, no static dependencies were found",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Copying app:rust-b without linking, no static dependencies were found",
    ]));

    // Build again with dynamic deps, now it should skip again
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());
    assert2::assert!(outputs.stdout_contains_ordered([
        "Linking dependencies",
        "  Found dynamic WASM RPC dependencies (app:rust-a, app:rust-b) for app:rust-a",
        "  Skipping linking dependencies for app:rust-a, UP-TO-DATE",
        "  Found dynamic WASM RPC dependencies (app:rust-a) for app:rust-b",
        "  Skipping linking dependencies for app:rust-b, UP-TO-DATE",
    ]));
}

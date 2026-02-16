use crate::app::{check_component_metadata, cmd, flag, pattern, TestContext};
use crate::Tracing;
use assert2::{assert, check};
use golem_cli::fs;
use golem_templates::model::GuestLanguage;
use indoc::indoc;
use std::path::Path;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
async fn app_help_in_empty_folder(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
}

#[test]
async fn app_new_with_many_components_and_then_help_in_app_folder(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx.cli([cmd::NEW, app_name, "typescript", "rust"]).await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "typescript", "app:typescript"])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
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
    let outputs = ctx.cli([cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success_or_dump());

    // First build
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stdout_contains("Compiling app_rust v0.0.1"));

    check_component_metadata(
        &ctx.working_dir
            .join("golem-temp/agents/app_rust_debug.wasm"),
        "app:rust".to_string(),
        None,
    );

    // Rebuild - 1
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 2
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 3 - force, but cargo is smart to skip actual compile
    let outputs = ctx.cli([cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stdout_contains("Finished `dev` profile"));

    // Rebuild - 4
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));

    // Clean
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    // Rebuild - 5
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stdout_contains("Compiling app_rust v0.0.1"));
}

#[test]
async fn app_new_language_hints(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::NEW, "dummy-app-name"]).await;
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
async fn basic_ifs_deploy(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success_or_dump());

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              app:rust:
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

        "},
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    check!(outputs.stdout_contains_ordered([
        "+      /Cargo.toml:",
        "+        permissions: read-only",
        "+      /src/lib.rs:",
        "+        permissions: read-write",
        "Planning",
        "- create component app:rust",
    ]));

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("app-rust")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              app:rust:
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

        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    check!(outputs.stdout_contains_ordered([
        "     filesByPath:",
        "-      /Cargo.toml:",
        "+      /Cargo2.toml:",
        "         permissions: read-only",
        "       /src/lib.rs:",
        "-        permissions: read-write",
        "+        permissions: read-only",
        "Planning",
        "- update component app:rust, changes:",
        "  - files",
        "    - delete file /Cargo.toml",
        "    - create file /Cargo2.toml",
        "    - update file /src/lib.rs, changes:",
        "      - permissions",
    ]));

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains(
        "Finished deployment planning, no changes are required for the environment [UP-TO-DATE]"
    ));
}

// TODO: atomic: re-enable IF we will have any builtin subcommands for golem app
#[ignore]
#[test]
async fn custom_app_subcommand_with_builtin_name() {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"])
        .await;
    assert!(outputs.success_or_dump());

    fs::append_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"

            customCommands:
              new:
                - command: cargo tree

        "},
    )
    .unwrap();

    let outputs = ctx.cli(cmd::NO_ARGS).await;
    assert!(!outputs.success());
    check!(outputs.stderr_contains(":new"));

    let outputs = ctx.cli([":new"]).await;
    assert!(outputs.success_or_dump());
    check!(outputs.stdout_contains("Executing external command 'cargo tree'"));
}

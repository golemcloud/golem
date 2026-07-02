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
async fn custom_rust_component_build_waits_for_guest_bridge_sdks(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:producer-{producer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&producer_extracted_agent_types),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: custom-rust-guest-bridge-order

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn manifest_dependency_without_generated_guest_target_does_not_block_dependent_build(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let component_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let component_wasm = component_wasm.to_str().unwrap();
    let helper_final_wasm = ctx.cwd_path_join("helper/helper-final.wasm");

    fs::create_dir_all(ctx.cwd_path_join("golem-temp/extracted-agent-types")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("helper")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();

    let helper_final_wasm_hash = blake3::hash(helper_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let helper_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:helper-{helper_final_wasm_hash}.json");
    let marker_hash = blake3::hash("app:helper".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:helper",
            "hashInput": "app:helper",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: manifest-dependency-without-generated-guest-target

            environments:
              local:
                server: local

            components:
              app:helper:
                templates: ts
                dir: helper
                buildMergeMode: replace
                componentWasm: helper-final.wasm
                outputWasm: helper-final.wasm
                build:
                  - command: cp {component_wasm} helper-final.wasm
                  - command: sh -c "printf '%s' '[]' > ../{helper_extracted_agent_types}"
                  - command: touch ../helper-built

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:helper
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../helper-built
                  - command: cp {component_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn manifest_guest_component_matcher_generates_for_selected_component_without_dependencies(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: explicit-guest-component-matcher-without-dependencies

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                guest:
                  agents: app:producer
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/rust-guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_enabled_for_rust_consumer(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_generates_for_rust_manifest_dependency(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-mixed-language-without-build-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_generates_for_rust_producer_used_by_rust_consumer(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_rust.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-rust-producer

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                buildMergeMode: replace
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_gen_bridge_step_includes_selected_dependencies(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_rust.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-gen-bridge-step

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                buildMergeMode: replace
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, "app:consumer", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_includes_producers_that_also_consume_guest_bridge(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let base_final_wasm = ctx.cwd_path_join("base-final.wasm");
    let middle_final_wasm = ctx.cwd_path_join("middle/middle-final.wasm");

    std::fs::copy(producer_wasm, &base_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:base-{base_final_wasm_hash}.json")),
        &base_agent_types,
    )
    .unwrap();

    let middle_final_wasm_hash = blake3::hash(middle_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let middle_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let middle_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:middle-{middle_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&middle_extracted_agent_types),
        &middle_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:base", "app:middle"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("middle")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-transitive-producer

            environments:
              local:
                server: local

            components:
              app:base:
                componentWasm: {producer_wasm}
                outputWasm: base-final.wasm

              app:middle:
                dir: middle
                dependencies:
                  - app:base
                componentWasm: middle.wasm
                outputWasm: middle-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} middle.wasm
                  - command: cp {producer_wasm} middle-final.wasm
                  - command: touch ../{middle_extracted_agent_types}

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:base
                  - app:middle
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_uses_manifest_dependencies_for_rust_consumers(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let base_final_wasm = ctx.cwd_path_join("base-final.wasm");
    let middle_final_wasm = ctx.cwd_path_join("middle/middle-final.wasm");

    std::fs::copy(producer_wasm, &base_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:base-{base_final_wasm_hash}.json")),
        &base_agent_types,
    )
    .unwrap();

    let middle_final_wasm_hash = blake3::hash(middle_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let middle_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let middle_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:middle-{middle_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&middle_extracted_agent_types),
        &middle_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:base", "app:middle"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("middle/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("middle/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "middle"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("middle/src/lib.rs"), "").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-rust-consumer

            environments:
              local:
                server: local

            components:
              app:base:
                componentWasm: {producer_wasm}
                outputWasm: base-final.wasm

              app:middle:
                templates: rust
                dir: middle
                dependencies:
                  - app:base
                buildMergeMode: replace
                componentWasm: middle.wasm
                outputWasm: middle-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} middle.wasm
                  - command: cp {producer_wasm} middle-final.wasm
                  - command: touch ../{middle_extracted_agent_types}

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:middle
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn selected_dependency_guest_bridge_uses_transitive_manifest_dependencies(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let base_final_wasm = ctx.cwd_path_join("base-final.wasm");
    let middle_final_wasm = ctx.cwd_path_join("middle/middle-final.wasm");

    std::fs::copy(producer_wasm, &base_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:base-{base_final_wasm_hash}.json")),
        &base_agent_types,
    )
    .unwrap();

    let middle_final_wasm_hash = blake3::hash(middle_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let middle_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let middle_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:middle-{middle_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&middle_extracted_agent_types),
        &middle_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:base", "app:middle"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("middle/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("middle/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "middle"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("middle/src/lib.rs"), "").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: selected-dependency-rust-guest-bridge-transitive-cargo

            environments:
              local:
                server: local

            components:
              app:base:
                componentWasm: {producer_wasm}
                outputWasm: base-final.wasm

              app:middle:
                templates: rust
                dir: middle
                dependencies:
                  - app:base
                buildMergeMode: replace
                componentWasm: middle.wasm
                outputWasm: middle-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} middle.wasm
                  - command: cp {producer_wasm} middle-final.wasm
                  - command: touch ../{middle_extracted_agent_types}

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:middle
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:consumer",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn selected_dependency_guest_bridge_only_generates_for_rust_dependency_paths(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let rust_producer_final_wasm = ctx.cwd_path_join("rust-producer-final.wasm");
    let ts_producer_final_wasm = ctx.cwd_path_join("ts-producer-final.wasm");

    std::fs::copy(producer_wasm, &rust_producer_final_wasm).unwrap();
    std::fs::copy(producer_wasm, &ts_producer_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let rust_producer_final_wasm_hash =
        blake3::hash(rust_producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let rust_producer_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!(
            "app:rust-producer-{rust_producer_final_wasm_hash}.json"
        )),
        &rust_producer_agent_types,
    )
    .unwrap();

    let ts_producer_final_wasm_hash =
        blake3::hash(ts_producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let ts_producer_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!(
            "app:ts-producer-{ts_producer_final_wasm_hash}.json"
        )),
        &ts_producer_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:rust-producer", "app:ts-producer"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("rust-consumer")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("ts-consumer")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: selected-dependency-rust-guest-bridge-disjoint-dependencies

            environments:
              local:
                server: local

            components:
              app:rust-producer:
                componentWasm: {producer_wasm}
                outputWasm: rust-producer-final.wasm

              app:ts-producer:
                componentWasm: {producer_wasm}
                outputWasm: ts-producer-final.wasm

              app:rust-consumer:
                templates: rust
                dir: rust-consumer
                dependencies:
                  - app:rust-producer
                buildMergeMode: replace
                componentWasm: rust-consumer.wasm
                outputWasm: rust-consumer-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} rust-consumer.wasm

              app:ts-consumer:
                templates: ts
                dir: ts-consumer
                dependencies:
                  - app:ts-producer
                buildMergeMode: replace
                componentWasm: ts-consumer.wasm
                outputWasm: ts-consumer-final.wasm
                build:
                  - command: cp {producer_wasm} ts-consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:rust-consumer",
            "app:ts-consumer",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
    assert!(
        !ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn selected_explicit_guest_bridge_uses_transitive_manifest_dependencies(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let base_final_wasm = ctx.cwd_path_join("base-final.wasm");

    std::fs::copy(producer_wasm, &base_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:base-{base_final_wasm_hash}.json")),
        &base_agent_types,
    )
    .unwrap();

    let marker_hash = blake3::hash("app:base".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:base",
            "hashInput": "app:base",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("middle/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("middle/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "middle"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("middle/src/lib.rs"), "").unwrap();
    fs::write_str(ctx.cwd_path_join("middle/package.json"), "{}\n").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: selected-explicit-rust-guest-bridge-transitive

            environments:
              local:
                server: local

            components:
              app:base:
                componentWasm: {producer_wasm}
                outputWasm: base-final.wasm

              app:middle:
                templates: rust
                dir: middle
                dependencies:
                  - app:base
                buildMergeMode: replace
                componentWasm: middle.wasm
                outputWasm: middle-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} middle.wasm
                  - command: cp {producer_wasm} middle-final.wasm

              app:consumer:
                dir: consumer
                dependencies:
                  - app:middle
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: app:base
                  outputDir: golem-temp/bridge-sdk/rust/guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:consumer",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn selected_explicit_guest_bridge_ignores_matcher_outside_effective_dependencies(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: selected-explicit-rust-guest-bridge-unrelated

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: app:producer
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:consumer",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("bridge/rust-guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_does_not_infer_rust_guest_target_from_cargo_files(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/package.json"), "{}\n").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-no-cargo-inference

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: ts
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_ignores_unselected_manifest_guest_targets(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("prebuilt/bar-agent-guest-client")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("prebuilt/bar-agent-guest-client/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "bar-agent-guest-client"
            version = "0.1.0"
            edition = "2021"
        "#},
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../prebuilt/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-unselected-target

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../prebuilt/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: app:producer
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, "app:consumer", flag::STEP, "build"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_external_bridge_does_not_block_prebuilt_guest_client_build(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();

    std::fs::copy(producer_wasm, ctx.cwd_path_join("producer-final.wasm")).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("prebuilt/bar-agent-guest-client")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("prebuilt/bar-agent-guest-client/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "bar-agent-guest-client"
            version = "0.1.0"
            edition = "2021"
        "#},
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../prebuilt/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: external-bridge-with-prebuilt-guest-client

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../prebuilt/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                agents: app:producer
                outputDir: bridge/external
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_builds_rust_consumers_after_post_build_guest_clients(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let seed_final_wasm = ctx.cwd_path_join("seed-final.wasm");
    let base_final_wasm = ctx.cwd_path_join("base/base-final.wasm");

    std::fs::copy(producer_wasm, &seed_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let seed_final_wasm_hash = blake3::hash(seed_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let seed_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:seed-{seed_final_wasm_hash}.json")),
        &seed_agent_types,
    )
    .unwrap();

    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let base_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:base-{base_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&base_extracted_agent_types),
        &base_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:seed", "app:base"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("base/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("base/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "base"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("base/src/lib.rs"), "").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("middle/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("middle/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "middle"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("middle/src/lib.rs"), "").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-rust-post-build-consumer

            environments:
              local:
                server: local

            components:
              app:seed:
                componentWasm: {producer_wasm}
                outputWasm: seed-final.wasm

              app:base:
                templates: rust
                dir: base
                dependencies:
                  - app:seed
                buildMergeMode: replace
                componentWasm: base.wasm
                outputWasm: base-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} base.wasm
                  - command: cp {producer_wasm} base-final.wasm
                  - command: touch ../{base_extracted_agent_types}

              app:middle:
                templates: rust
                dir: middle
                dependencies:
                  - app:base
                buildMergeMode: replace
                componentWasm: middle.wasm
                outputWasm: middle-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} middle.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:base
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_waits_for_unseeded_producer_consumers(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let seed_final_wasm = ctx.cwd_path_join("seed-final.wasm");
    let base_final_wasm = ctx.cwd_path_join("base/base-final.wasm");

    std::fs::copy(producer_wasm, &seed_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let seed_final_wasm_hash = blake3::hash(seed_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let seed_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:seed-{seed_final_wasm_hash}.json")),
        &seed_agent_types,
    )
    .unwrap();

    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("base-agent-types.json"),
        &base_agent_types,
    )
    .unwrap();
    let base_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:base-{base_final_wasm_hash}.json");

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:seed", "app:base"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("base/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("base/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "base"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("base/src/lib.rs"), "").unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-unseeded-producer-consumer

            environments:
              local:
                server: local

            components:
              app:seed:
                componentWasm: {producer_wasm}
                outputWasm: seed-final.wasm

              app:base:
                templates: rust
                dir: base
                dependencies:
                  - app:seed
                buildMergeMode: replace
                componentWasm: base.wasm
                outputWasm: base-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} base.wasm
                  - command: cp {producer_wasm} base-final.wasm
                  - command: cp ../base-agent-types.json ../{base_extracted_agent_types}

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:base
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} consumer.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn dependency_guest_bridge_counts_explicit_pre_build_clients_when_scheduling_producer_consumers(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let seed_final_wasm = ctx.cwd_path_join("seed-final.wasm");
    let base_final_wasm = ctx.cwd_path_join("base/base-final.wasm");

    std::fs::copy(producer_wasm, &seed_final_wasm).unwrap();

    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let extracted_agent_types = serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let seed_final_wasm_hash = blake3::hash(seed_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let seed_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:seed-{seed_final_wasm_hash}.json")),
        &seed_agent_types,
    )
    .unwrap();

    let base_final_wasm_hash = blake3::hash(base_final_wasm.display().to_string().as_bytes())
        .to_hex()
        .to_string();
    let base_agent_types = serde_json::to_string(
        &extracted_agent_types
            .iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("base-agent-types.json"),
        &base_agent_types,
    )
    .unwrap();
    let base_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:base-{base_final_wasm_hash}.json");

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:seed", "app:base"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("base/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("base/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "base"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("base/src/lib.rs"), "").unwrap();
    fs::write_str(ctx.cwd_path_join("base/package.json"), "{}\n").unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            foo-agent-guest-client = { path = "../golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-rust-guest-bridge-explicit-pre-build-producer-consumer

            environments:
              local:
                server: local

            components:
              app:seed:
                componentWasm: {producer_wasm}
                outputWasm: seed-final.wasm

              app:base:
                templates: ts
                dir: base
                dependencies:
                  - app:seed
                buildMergeMode: replace
                componentWasm: base.wasm
                outputWasm: base-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} base.wasm
                  - command: cp {producer_wasm} base-final.wasm
                  - command: cp ../base-agent-types.json ../{base_extracted_agent_types}

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:base
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: golem-temp/bridge-sdk/rust/guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("golem-temp/bridge-sdk/rust/guest/foo-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn custom_build_env_guest_bridge_path_waits_for_guest_bridge_sdks(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:producer-{producer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&producer_extracted_agent_types),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: custom-env-guest-bridge-order

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: sh -c 'test -f "$SDK_PATH"'
                    env:
                      SDK_PATH: ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_shell_command_literal_guest_bridge_path_waits_for_guest_bridge_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: shell-literal-guest-bridge-order

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: sh -c 'test -f ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml'
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn dependency_guest_bridge_build_targets_do_not_make_component_require_guest_bridge(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer/producer-final.wasm");

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:producer-{producer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&producer_extracted_agent_types),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("producer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: build-target-is-not-guest-bridge-input

            environments:
              local:
                server: local

            components:
              app:producer:
                dir: producer
                componentWasm: producer.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: cp {producer_wasm} producer.wasm
                    targets:
                      - ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} producer-final.wasm
                  - command: touch ../{producer_extracted_agent_types}
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("bridge/rust-guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn rust_cargo_path_guest_bridge_dependency_waits_for_guest_bridge_sdks(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "../bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-cargo-path-guest-bridge-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                    sources:
                      - Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_manifest_path_cargo_guest_bridge_dependency_waits_for_guest_bridge_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar-agent-guest-client = { path = "bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("src/lib.rs"), "").unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer-component"
            version = "0.1.0"
            edition = "2021"
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-manifest-path-guest-bridge-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --manifest-path ../Cargo.toml --format-version=1
                    sources:
                      - ../Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_target_specific_cargo_path_guest_bridge_dependency_waits_for_guest_bridge_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [target.wasm32-wasip2.dependencies]
            bar-agent-guest-client = { path = "../bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-target-specific-cargo-path-guest-bridge-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                    sources:
                      - Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_workspace_cargo_path_guest_bridge_dependency_waits_for_guest_bridge_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            members = ["consumer"]
            resolver = "2"

            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar = { workspace = true }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-workspace-cargo-path-guest-bridge-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                    sources:
                      - Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_workspace_multiline_guest_bridge_dependency_waits_for_guest_bridge_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            members = ["consumer"]
            resolver = "2"

            [workspace.dependencies]
            bar = {
                package = "bar-agent-guest-client",
                path = "bridge/bar-agent-guest-client",
            }
        "#},
    )
    .unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar = { workspace = true }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-workspace-multiline-guest-bridge-dependency

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                    sources:
                      - Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_workspace_multiline_dependency_use_waits_for_guest_bridge_sdks(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            members = ["consumer"]
            resolver = "2"

            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("consumer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "consumer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            bar = {
                workspace = true,
            }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-workspace-multiline-dependency-use

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                dir: consumer
                dependencies:
                  - app:producer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: cargo metadata --format-version=1
                    sources:
                      - Cargo.toml
                      - src
                    targets:
                      - consumer.wasm
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_workspace_dependency_name_prefix_does_not_make_guest_bridge_consumer(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer/producer-final.wasm");

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            members = ["producer"]
            resolver = "2"

            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "bridge/bar-agent-guest-client" }
            barista = "0.0.1"
        "#},
    )
    .unwrap();
    fs::create_dir_all(ctx.cwd_path_join("producer/src")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("producer/Cargo.toml"),
        indoc! {r#"
            [package]
            name = "producer"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            barista = { workspace = true }
        "#},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("producer/src/lib.rs"), "").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-workspace-dependency-prefix

            environments:
              local:
                server: local

            components:
              app:producer:
                dir: producer
                templates: rust
                buildMergeMode: replace
                componentWasm: producer-final.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: cp {producer_wasm} producer-final.wasm
                  - command: touch ../golem-temp/extracted-agent-types/app:producer-{producer_final_wasm_hash}.json

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_no_build_producer_generates_guest_bridge_before_consumer_build(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();
    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [package]
            name = "prebuilt-rust-producer"
            version = "0.0.1"
            edition = "2021"
        "#},
    )
    .unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-no-build-producer-guest-bridge-order

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn custom_unknown_language_build_producer_generates_guest_bridge(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_dir = ctx.cwd_path_join("producer");
    let producer_final_wasm = producer_dir.join("producer-final.wasm");

    fs::create_dir_all(&producer_dir).unwrap();
    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: custom-unknown-language-guest-bridge-source

            environments:
              local:
                server: local

            components:
              app:producer:
                dir: producer
                componentWasm: producer.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: cp {producer_wasm} producer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/rust-guest/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn custom_unknown_language_build_consumer_waits_for_custom_guest_bridge_output_dir(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: custom-unknown-language-custom-guest-output-dir

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../sdk/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: sdk
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_guest_bridge_choreography_keeps_builds_before_metadata(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let component_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let component_wasm = component_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("a")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("b")).unwrap();
    fs::write_str(ctx.cwd_path_join("a/package.json"), "{}").unwrap();
    fs::write_str(ctx.cwd_path_join("b/package.json"), "{}").unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-guest-step-order

            environments:
              local:
                server: local

            components:
              app:a:
                dir: a
                componentWasm: a.wasm
                outputWasm: a-final.wasm
                build:
                  - command: cp {component_wasm} a.wasm

              app:b:
                dir: b
                componentWasm: b.wasm
                outputWasm: b-final.wasm
                build:
                  - command: test ! -f ../a/a-final.wasm
                  - command: cp {component_wasm} b.wasm

            bridge:
              rust:
                guest:
                  agents: app:a
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "add-metadata"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_template_no_build_producer_generates_guest_bridge_before_consumer_build(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-template-no-build-producer-guest-bridge-order

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                buildMergeMode: replace
                build: []
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/rust-guest/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_build_that_does_not_use_guest_bridge_can_generate_own_guest_bridge(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-self-guest-bridge-after-build

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                buildMergeMode: replace
                componentWasm: producer-final.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: cp {producer_wasm} producer-final.wasm
                  - command: touch -t 200001010000 producer-final.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge/rust-guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn rust_dev_dependency_on_guest_bridge_does_not_make_build_a_guest_bridge_consumer(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("Cargo.toml"),
        indoc! {r#"
            [package]
            name = "producer"
            version = "0.1.0"
            edition = "2021"

            [dev-dependencies]
            bar-agent-guest-client = { path = "bridge/bar-agent-guest-client" }
        "#},
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-dev-dependency-is-not-build-input

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                buildMergeMode: replace
                componentWasm: producer-final.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: cp {producer_wasm} producer-final.wasm
                  - command: touch -t 200001010000 producer-final.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn guest_and_external_bridge_output_dir_overlap_is_rejected(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        fs::read_to_string(
            crate::crate_path()
                .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-bridge-output-dirs

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                agents: BarAgent
                outputDir: bridge
                guest:
                  agents: BarAgent
                  outputDir: bridge/bar-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD, flag::STEP, "gen-bridge"]).await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("bridge/bar-agent-client/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn guest_bridge_output_dir_overlap_with_default_external_dir_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let consumer_final_wasm = ctx.cwd_path_join("consumer/consumer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();

    let consumer_final_wasm_hash =
        blake3::hash(consumer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let consumer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let consumer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:consumer-{consumer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&consumer_extracted_agent_types),
        &consumer_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:producer", "app:consumer"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: default-external-overlaps-guest-output-dir

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../golem-temp/bridge-sdk/rust/bar-agent-client/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm
                  - command: touch ../{consumer_extracted_agent_types}

            bridge:
              rust:
                agents: BarAgent
                guest:
                  agents: BarAgent
                  outputDir: golem-temp/bridge-sdk/rust/bar-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join(
            "golem-temp/bridge-sdk/rust/bar-agent-client/bar-agent-guest-client/Cargo.toml"
        )
        .exists()
    );
}

#[test]
async fn guest_and_external_bridge_same_output_dir_base_generates_sibling_sdks(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: shared-bridge-output-dir-base

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                agents: BarAgent
                outputDir: bridge
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn component_matcher_guest_and_external_same_output_dir_base_generates_sibling_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: shared-bridge-output-dir-base-component-matcher

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                agents: app:producer
                outputDir: bridge
                guest:
                  agents: app:producer
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn component_matcher_same_output_dir_base_with_guest_consumer_generates_sibling_sdks(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: shared-bridge-output-dir-base-component-matcher-with-consumer

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm

            bridge:
              rust:
                agents: app:producer
                outputDir: bridge
                guest:
                  agents: app:producer
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-client/Cargo.toml")
            .exists()
    );
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn post_build_guest_bridge_scan_rejects_duplicate_agent_output_dir(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let consumer_final_wasm = ctx.cwd_path_join("consumer/consumer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let bar_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &bar_agent_types,
    )
    .unwrap();

    let consumer_final_wasm_hash =
        blake3::hash(consumer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let consumer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:consumer-{consumer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&consumer_extracted_agent_types),
        &bar_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:producer", "app:consumer"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: duplicate-post-build-guest-agent
            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm
                  - command: touch ../{consumer_extracted_agent_types}

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
}

#[test]
async fn post_build_guest_bridge_scan_rejects_newly_built_duplicate_agent_output_dir(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let bar_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &bar_agent_types,
    )
    .unwrap();

    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: newly-built-duplicate-post-build-guest-agent
            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                dir: consumer
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../bridge/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
}

#[test]
async fn unselected_external_bridge_output_dir_does_not_block_selected_guest_bridge(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: unselected-external-bridge-output-dir

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                componentWasm: {producer_wasm}
                outputWasm: consumer-final.wasm

            bridge:
              rust:
                agents: app:consumer
                outputDir: bridge/bar-agent-guest-client
                guest:
                  agents: BarAgent
                  outputDir: bridge
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:producer",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        ctx.cwd_path_join("bridge/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn external_bridge_missing_agent_matcher_is_rejected_with_guest_scheduler(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: missing-external-bridge-agent-matcher

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                agents: MissingAgent
                outputDir: bridge/external
                guest:
                  agents: BarAgent
                  outputDir: bridge/guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
}

#[test]
async fn manifest_dependency_orders_plain_component_build_without_guest_bridge(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let component_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let component_wasm = component_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("helper")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: dependency-ordered-plain-build

            environments:
              local:
                server: local

            components:
              app:consumer:
                dir: consumer
                dependencies:
                  - app:helper
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../helper-built
                  - command: cp {component_wasm} consumer.wasm

              app:helper:
                dir: helper
                buildMergeMode: replace
                componentWasm: helper.wasm
                outputWasm: helper-final.wasm
                build:
                  - command: touch ../helper-built
                  - command: cp {component_wasm} helper.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn selected_manifest_dependency_orders_plain_component_build_without_guest_bridge(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let component_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let component_wasm = component_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("helper")).unwrap();
    fs::create_dir_all(ctx.cwd_path_join("consumer")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: selected-dependency-ordered-plain-build

            environments:
              local:
                server: local

            components:
              app:consumer:
                dir: consumer
                dependencies:
                  - app:helper
                buildMergeMode: replace
                componentWasm: consumer.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f ../helper-built
                  - command: cp {component_wasm} consumer.wasm

              app:helper:
                dir: helper
                buildMergeMode: replace
                componentWasm: helper.wasm
                outputWasm: helper-final.wasm
                build:
                  - command: touch ../helper-built
                  - command: cp {component_wasm} helper.wasm
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, "app:consumer", flag::STEP, "build"])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn unselected_guest_component_matcher_does_not_create_selected_build_cycle(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();

    fs::create_dir_all(ctx.cwd_path_join("selected")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: unselected-guest-component-matcher

            environments:
              local:
                server: local

            components:
              app:selected:
                dir: selected
                componentWasm: selected.wasm
                outputWasm: selected-final.wasm
                build:
                  - command: echo guest-client is not needed for this selected build
                  - command: cp {producer_wasm} selected.wasm

              app:unselected:
                componentWasm: {producer_wasm}
                outputWasm: unselected-final.wasm

            bridge:
              rust:
                guest:
                  agents: app:unselected
                  outputDir: bridge/guest
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            "app:selected",
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
        ])
        .await;
    assert!(outputs.success_or_dump());
}

#[test]
async fn guest_and_repl_bridge_output_dir_overlap_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-repl-bridge-output-dirs

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: golem-temp/repl/rust/bridge-sdk/bar-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            flag::STEP,
            "gen-bridge",
            "--repl-bridge-sdk-target",
            "rust",
        ])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join(
            "golem-temp/repl/rust/bridge-sdk/bar-agent-client/bar-agent-guest-client/Cargo.toml"
        )
        .exists()
    );
}

#[test]
async fn guest_and_repl_bridge_output_dir_overlap_with_different_agent_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let repl_source_final_wasm = ctx.cwd_path_join("repl-source-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();
    std::fs::copy(producer_wasm, &repl_source_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let repl_source_final_wasm_hash =
        blake3::hash(repl_source_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let repl_source_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!(
            "app:repl-source-{repl_source_final_wasm_hash}.json"
        )),
        &repl_source_agent_types,
    )
    .unwrap();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:producer", "app:repl-source"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-repl-bridge-output-dirs-different-agent

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:repl-source:
                componentWasm: {producer_wasm}
                outputWasm: repl-source-final.wasm

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: golem-temp/repl/rust/bridge-sdk/foo-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            flag::STEP,
            "gen-bridge",
            "--repl-bridge-sdk-target",
            "rust",
        ])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join(
            "golem-temp/repl/rust/bridge-sdk/foo-agent-client/bar-agent-guest-client/Cargo.toml"
        )
        .exists()
    );
}

#[test]
async fn guest_and_build_produced_repl_bridge_output_dir_overlap_with_different_agent_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let consumer_final_wasm = ctx.cwd_path_join("consumer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let consumer_final_wasm_hash =
        blake3::hash(consumer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let consumer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let consumer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:consumer-{consumer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&consumer_extracted_agent_types),
        &consumer_agent_types,
    )
    .unwrap();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:producer", "app:consumer"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-build-produced-repl-bridge-output-dirs-different-agent

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                buildMergeMode: replace
                componentWasm: consumer-final.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f golem-temp/repl/rust/bridge-sdk/foo-agent-client/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm
                  - command: touch {consumer_extracted_agent_types}

            bridge:
              rust:
                guest:
                  agents: BarAgent
                  outputDir: golem-temp/repl/rust/bridge-sdk/foo-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::BUILD,
            flag::STEP,
            "build",
            flag::STEP,
            "gen-bridge",
            "--repl-bridge-sdk-target",
            "rust",
        ])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join(
            "golem-temp/repl/rust/bridge-sdk/foo-agent-client/bar-agent-guest-client/Cargo.toml"
        )
        .exists()
    );
}

#[test]
async fn guest_and_rust_consumer_external_bridge_output_dir_overlap_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let consumer_final_wasm = ctx.cwd_path_join("consumer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();
    std::fs::copy(producer_wasm, &consumer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let consumer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let consumer_final_wasm_hash =
        blake3::hash(consumer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:consumer-{consumer_final_wasm_hash}.json")),
        &consumer_agent_types,
    )
    .unwrap();
    let marker_hash = blake3::hash("app:producer".as_bytes()).to_hex().to_string();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:producer",
            "hashInput": "app:producer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();
    let marker_hash = blake3::hash("app:consumer".as_bytes()).to_hex().to_string();
    fs::write_str(
        task_results_dir.join(&marker_hash),
        serde_json::to_string(&serde_json::json!({
            "kind": "ExtractAgentTypeMarkerHash",
            "id": "app:consumer",
            "hashInput": "app:consumer",
            "hashHex": marker_hash,
            "success": true,
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-bridge-output-dirs-rust-consumer

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                buildMergeMode: replace
                componentWasm: consumer-final.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f bridge/bar-agent-client/bar-agent-guest-client/Cargo.toml

            bridge:
              rust:
                agents: app:consumer
                outputDir: bridge/bar-agent-client/bar-agent-guest-client
                guest:
                  agents: BarAgent
                  outputDir: bridge/bar-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("bridge/bar-agent-client/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn guest_and_build_produced_external_bridge_output_dir_overlap_is_rejected_before_generation(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    let producer_wasm = producer_wasm.to_str().unwrap();
    let producer_final_wasm = ctx.cwd_path_join("producer-final.wasm");
    let consumer_final_wasm = ctx.cwd_path_join("consumer-final.wasm");

    std::fs::copy(producer_wasm, &producer_final_wasm).unwrap();

    let extracted_agent_types_dir = ctx.cwd_path_join("golem-temp/extracted-agent-types");
    fs::create_dir_all(&extracted_agent_types_dir).unwrap();
    let extracted_agent_types = fs::read_to_string(
        crate::crate_path()
            .join("test-data/goldenfiles/extracted-agent-types/code_first_snippets_ts.json"),
    )
    .unwrap();
    let producer_final_wasm_hash =
        blake3::hash(producer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let producer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "BarAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write_str(
        extracted_agent_types_dir.join(format!("app:producer-{producer_final_wasm_hash}.json")),
        &producer_agent_types,
    )
    .unwrap();
    let consumer_final_wasm_hash =
        blake3::hash(consumer_final_wasm.display().to_string().as_bytes())
            .to_hex()
            .to_string();
    let consumer_agent_types = serde_json::to_string(
        &serde_json::from_str::<Vec<JsonValue>>(&extracted_agent_types)
            .unwrap()
            .into_iter()
            .filter(|agent_type| agent_type["type_name"] == "FooAgent")
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let consumer_extracted_agent_types =
        format!("golem-temp/extracted-agent-types/app:consumer-{consumer_final_wasm_hash}.json");
    fs::write_str(
        ctx.cwd_path_join(&consumer_extracted_agent_types),
        &consumer_agent_types,
    )
    .unwrap();
    let task_results_dir = ctx.cwd_path_join("golem-temp/task-results");
    fs::create_dir_all(&task_results_dir).unwrap();
    for component_name in ["app:producer", "app:consumer"] {
        let marker_hash = blake3::hash(component_name.as_bytes()).to_hex().to_string();
        fs::write_str(
            task_results_dir.join(&marker_hash),
            serde_json::to_string(&serde_json::json!({
                "kind": "ExtractAgentTypeMarkerHash",
                "id": component_name,
                "hashInput": component_name,
                "hashHex": marker_hash,
                "success": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: overlapping-build-produced-bridge-output-dirs

            environments:
              local:
                server: local

            components:
              app:producer:
                componentWasm: {producer_wasm}
                outputWasm: producer-final.wasm

              app:consumer:
                templates: rust
                buildMergeMode: replace
                componentWasm: consumer-final.wasm
                outputWasm: consumer-final.wasm
                build:
                  - command: test -f bridge/bar-agent-client/bar-agent-guest-client/Cargo.toml
                  - command: cp {producer_wasm} consumer-final.wasm
                  - command: touch {consumer_extracted_agent_types}

            bridge:
              rust:
                agents: app:consumer
                outputDir: bridge/bar-agent-client/bar-agent-guest-client
                guest:
                  agents: BarAgent
                  outputDir: bridge/bar-agent-client
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx
        .cli([cmd::BUILD, flag::STEP, "build", flag::STEP, "gen-bridge"])
        .await;
    assert!(!outputs.success_or_dump());
    assert!(
        !ctx.cwd_path_join("bridge/bar-agent-client/bar-agent-guest-client/Cargo.toml")
            .exists()
    );
}

#[test]
async fn rust_guest_bridge_matcher_without_non_rust_component_is_rejected(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    std::fs::copy(producer_wasm, ctx.cwd_path_join("producer-final.wasm")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-guest-unmatched-agent

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                componentWasm: producer.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: "true"

            bridge:
              rust:
                guest:
                  agents: MissingAgent
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD, flag::STEP, "gen-bridge"]).await;
    assert!(!outputs.success_or_dump());
}

#[test]
async fn rust_guest_bridge_component_matcher_without_non_rust_component_is_rejected(
    _tracing: &Tracing,
) {
    let ctx = TestContext::new();
    let producer_wasm = crate::workspace_path().join("test-components/golem_it_agent_rpc.wasm");
    std::fs::copy(producer_wasm, ctx.cwd_path_join("producer-final.wasm")).unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: rust-guest-unmatched-component

            environments:
              local:
                server: local

            components:
              app:producer:
                templates: rust
                componentWasm: producer.wasm
                outputWasm: producer-final.wasm
                build:
                  - command: "true"

            bridge:
              rust:
                guest:
                  agents: app:producer
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD, flag::STEP, "gen-bridge"]).await;
    assert!(!outputs.success_or_dump());
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
    package_json["devDependencies"]["@golemcloud/golem-ts-typegen"] =
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
    tsconfig["compilerOptions"]["experimentalDecorators"] = JsonValue::Bool(false);
    tsconfig["compilerOptions"]["emitDecoratorMetadata"] = JsonValue::Bool(false);
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
            "dependencies": {},
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
async fn build_check_includes_selected_manifest_dependencies(_tracing: &Tracing) {
    let app_name = "test-app-check-dependencies";

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

    let ts_component_name = format!("{app_name}:ts-main");
    let rust_component_name = format!("{app_name}:rust-main");
    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let updated_manifest = manifest.replace(
        &format!("  {ts_component_name}:\n    dir: \"ts-main\"\n    templates: ts"),
        &format!(
            "  {ts_component_name}:\n    dir: \"ts-main\"\n    templates: ts\n    dependencies:\n      - {rust_component_name}"
        ),
    );
    assert_ne!(manifest, updated_manifest);
    fs::write_str(&manifest_path, updated_manifest).unwrap();

    let cargo_toml_path = ctx.cwd_path_join("rust-main/Cargo.toml");
    let mut cargo_toml: DocumentMut = fs::read_to_string(&cargo_toml_path)
        .unwrap()
        .parse()
        .unwrap();
    cargo_toml["dependencies"]["golem-rust"] = value("999.0.0");
    fs::write_str(&cargo_toml_path, cargo_toml.to_string()).unwrap();

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::BUILD,
            ts_component_name.as_str(),
            flag::STEP,
            "check",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(
        outputs.stdout_contains("Planned required changes for dependencies and configurations")
    );

    let cargo_toml: DocumentMut = fs::read_to_string(&cargo_toml_path)
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(cargo_toml["dependencies"]["golem-rust"].as_str(), Some("999.0.0"));
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
            import {
                BaseAgent,
                agent,
                prompt,
                description
            } from '@golemcloud/golem-ts-sdk';

            @agent({})
            class SampleAgent extends BaseAgent {
                private readonly name: string;
                private readonly region: string;
                private readonly mode: "fast" | "safe";
                private readonly complex: { a: number, b: string };
                private value: number = 0;

                constructor(name: string, region: string, mode: "fast" | "safe", complex?: { a: number, b: string }) {
                    super()
                    this.name = name;
                    this.region = region;
                    this.mode = mode;
                    this.complex = complex ?? { a: 0, b: "default" };
                }

                @prompt("Run sample method")
                @description("Runs the sample method and returns the new value")
                async sampleMethod(): Promise<number> {
                    this.value += 1;
                    return this.value;
                }
            }
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("main.ts")),
        indoc! {
            r#"
            export * from './counter-agent';
            export * from './sample-agent';
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
            import {
                agent,
                BaseAgent,
                prompt,
                description,
                Config,
                Secret,
            } from '@golemcloud/golem-ts-sdk';

            type CounterAgentConfigV1 = {
                value: number;
            };

            @agent()
            class CounterAgent extends BaseAgent {
                count = 0;

                constructor(name: string, readonly config: Config<CounterAgentConfigV1>) {
                    super();
                }

                @prompt("Increase the count by the configured amount")
                @description("Increases the count and returns the new value")
                async increment(): Promise<number> {
                    this.count += this.config.value.value;
                    return this.count;
                }
            }

            export { CounterAgent };
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
            import {
                agent,
                BaseAgent,
                prompt,
                description,
                Config,
                Secret,
            } from '@golemcloud/golem-ts-sdk';

            type CounterAgentConfigV2 = {
                value: boolean;
            };

            @agent()
            class CounterAgent extends BaseAgent {
                count = 0;

                constructor(name: string, readonly config: Config<CounterAgentConfigV2>) {
                    super();
                }

                @prompt("Increase the count based on the configured boolean")
                @description("Increases the count and returns the new value")
                async increment(): Promise<number> {
                    this.count += this.config.value.value ? 1 : 0;
                    return this.count;
                }
            }

            export { CounterAgent };
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
            import {
                agent,
                BaseAgent,
                prompt,
                description,
                Config,
                Secret,
            } from '@golemcloud/golem-ts-sdk';

            type CounterAgentConfigV3 = {
                value: boolean;
                secret: Secret<string>;
            };

            @agent()
            class CounterAgent extends BaseAgent {
                count = 0;

                constructor(name: string, readonly config: Config<CounterAgentConfigV3>) {
                    super();
                }

                @prompt("Increase the count using the configured secret")
                @description("Increases the count and returns the new value")
                async increment(): Promise<number> {
                    const config = this.config.value;
                    this.count += config.value ? config.secret.get().length : 0;
                    return this.count;
                }
            }

            export { CounterAgent };
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
            import {
                agent,
                BaseAgent,
                prompt,
                description,
                Config,
                Secret,
            } from '@golemcloud/golem-ts-sdk';

            type CounterAgentConfigV4 = {
                value: boolean;
                secret: Secret<number>;
            };

            @agent()
            class CounterAgent extends BaseAgent {
                count = 0;

                constructor(name: string, readonly config: Config<CounterAgentConfigV4>) {
                    super();
                }

                @prompt("Increase the count using the configured numeric secret")
                @description("Increases the count and returns the new value")
                async increment(): Promise<number> {
                    const config = this.config.value;
                    this.count += config.value ? config.secret.get() : 0;
                    return this.count;
                }
            }

            export { CounterAgent };
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
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import { readFileSync } from 'node:fs';

            @agent({})
            class CounterAgent extends BaseAgent {
                constructor(name: string) {
                    super();
                }

                async increment(): Promise<number> {
                    return 1;
                }

                async readFile(path: string): Promise<string> {
                    return readFileSync(path, 'utf8');
                }
            }
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("sample-agent.ts")),
        indoc! {
            r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import { readFileSync } from 'node:fs';

            @agent({})
            class SampleAgent extends BaseAgent {
                constructor(name: string) {
                    super();
                }

                async sampleMethod(): Promise<number> {
                    return 1;
                }

                async readFile(path: string): Promise<string> {
                    return readFileSync(path, 'utf8');
                }
            }
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("main.ts")),
        indoc! {
            r#"
            export const SHARED_IFS_MARKER = 'ifs-multi-agent-marker';
            export * from './counter-agent';
            export * from './sample-agent';
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

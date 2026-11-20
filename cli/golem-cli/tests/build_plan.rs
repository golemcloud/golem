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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use test_r::test;

fn cli_command() -> Command {
    Command::new(crate::app::test_binary_path(
        &crate::app::test_binary_profile(),
        "golem",
    ))
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    if !root.exists() {
        return files;
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.unwrap();
        let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
        let content = if entry.file_type().is_symlink() {
            std::fs::read_link(entry.path())
                .unwrap()
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else if entry.file_type().is_file() {
            std::fs::read(entry.path()).unwrap()
        } else {
            Vec::new()
        };
        files.insert(relative, content);
    }
    files
}

#[cfg(target_os = "linux")]
#[test]
fn cold_go_toolchain_check_is_read_only_and_build_prepares_it() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "manifestVersion: 2.0.0\napp: cold-go\ncomponentTemplates:\n  local-go:\n    guestLanguage: go\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    templates: local-go\n    componentWasm: component.wasm\n    build:\n      - command: go version\n      - command: node build.cjs\n").unwrap();
    std::fs::write(
        project.path().join("build.cjs"),
        "require('node:fs').copyFileSync('seed.wasm', 'component.wasm');",
    )
    .unwrap();
    let seed = wat::parse_str(include_str!(
        "../../../test-components/size-analysis/retention.wat"
    ))
    .unwrap();
    std::fs::write(project.path().join("seed.wasm"), &seed).unwrap();
    let command = || {
        let mut command = cli_command();
        command
            .current_dir(project.path())
            .env_remove("GOLEM_GO_TOOLCHAIN")
            .env("XDG_CACHE_HOME", cache.path())
            .env("GOLEM_WASM_RPC_OFFLINE", "false")
            .arg("--config-dir")
            .arg(config.path());
        command
    };
    let before = snapshot(project.path());
    for yes in [false, true] {
        for offline in [false, true] {
            for _ in 0..2 {
                let mut command = command();
                if yes {
                    command.arg("--yes");
                }
                command.args(["build", "--step", "check"]);
                if offline {
                    command.env("GOLEM_WASM_RPC_OFFLINE", "true");
                }
                let output = command.output().unwrap();
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(output.status.success(), !offline, "{text}");
                if offline {
                    assert!(text.contains("offline mode is enabled"), "{text}");
                }
                assert_eq!(snapshot(project.path()), before, "{text}");
                assert_eq!(
                    snapshot(cache.path()),
                    BTreeMap::from([(PathBuf::new(), Vec::new())]),
                    "check must not install a toolchain: {text}"
                );
            }
        }
    }
    let refused = command()
        .env("GOLEM_WASM_RPC_OFFLINE", "true")
        .args(["--yes", "build", "--step", "build"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!refused.status.success(), "{text}");
    assert!(text.contains("offline mode is enabled"), "{text}");
    assert_eq!(snapshot(project.path()), before, "{text}");
    assert_eq!(
        snapshot(cache.path()),
        BTreeMap::from([(PathBuf::new(), Vec::new())])
    );

    for offline in [false, true] {
        let mut command = command();
        command.args(["--yes", "build", "--step", "build"]);
        if offline {
            command.env("GOLEM_WASM_RPC_OFFLINE", "true");
        }
        let output = command.output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        let release = golem_cli::versions::go_toolchain::TAG
            .split('-')
            .next()
            .unwrap();
        assert!(text.contains(&format!("go version {release}")), "{text}");
        assert_eq!(
            std::fs::read(project.path().join("component.wasm")).unwrap(),
            seed
        );
        assert_eq!(text.contains("Installing"), !offline, "{text}");
    }
}

#[test]
fn build_check_does_not_upgrade_manifests_even_with_yes() {
    for yes in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("golem.yaml"),
            "manifestVersion: 1.6.0\napp: check-upgrade\nincludes: [included.yaml]\nenvironments:\n  local:\n    default: true\n    server: local\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n    outputWasm: staged.wasm\n").unwrap();
        std::fs::write(
            project.path().join("included.yaml"),
            "componentTemplates:\n  shared:\n    outputWasm: inherited.wasm\n",
        )
        .unwrap();
        std::fs::write(project.path().join("component.wasm"), b"unchanged artifact").unwrap();
        let before = snapshot(project.path());
        let mut command = cli_command();
        command
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path());
        if yes {
            command.arg("--yes");
        }
        let output = command.args(["build", "--step", "check"]).output().unwrap();
        assert!(
            output.status.success(),
            "check inspects preparation without applying it: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            snapshot(project.path()),
            before,
            "check must not upgrade or repair a project, even with --yes; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Applying"));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains("no executable build plan was constructed"),
            "{text}"
        );
        assert!(!text.contains("checked host build stages"), "{text}");

        let build = cli_command()
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "build"])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "ordinary confirmed builds still upgrade: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let manifest = std::fs::read_to_string(project.path().join("golem.yaml")).unwrap();
        assert!(manifest.contains("manifestVersion: 2.0.0"));
        assert!(!manifest.contains("outputWasm"));
        assert_eq!(
            std::fs::read(project.path().join("component.wasm")).unwrap(),
            b"unchanged artifact"
        );
    }
}

#[test]
fn authorized_preparation_is_persisted_before_a_failing_build_recipe() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "manifestVersion: 1.6.0\napp: preparation-boundary\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n    outputWasm: obsolete.wasm\n    build:\n      - command: node inspect-prepared.cjs\n").unwrap();
    std::fs::write(
        project.path().join("inspect-prepared.cjs"),
        r#"
const fs = require('node:fs');
const manifest = fs.readFileSync('golem.yaml', 'utf8');
if (!manifest.includes('manifestVersion: 2.0.0') || manifest.includes('outputWasm')) {
    throw new Error('recipe received unprepared project files');
}
fs.writeFileSync('observed.yaml', manifest);
throw new Error('expected failure after preparation');
"#,
    )
    .unwrap();
    let output = cli_command()
        .current_dir(project.path())
        .arg("--config-dir")
        .arg(config.path())
        .args(["--yes", "build", "--step", "build"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("expected failure after preparation"),
        "{text}"
    );
    let persisted = std::fs::read(project.path().join("golem.yaml")).unwrap();
    assert_eq!(
        std::fs::read(project.path().join("observed.yaml")).unwrap(),
        persisted
    );
    assert!(
        String::from_utf8(persisted)
            .unwrap()
            .contains("manifestVersion: 2.0.0")
    );
    assert!(!project.path().join("component.wasm").exists());
}

#[test]
fn stale_common_markers_survive_checks_and_requirement_refusals() {
    use golem_cli::app::build::task_result_marker::TaskResult;
    use golem_cli::app::template::AppTemplateRepo;
    use golem_cli::model::app::Application;
    use golem_cli::model::language::GuestLanguage;
    use golem_common::base_model::application::ApplicationName;

    let template = AppTemplateRepo::get(false)
        .unwrap()
        .common_on_demand_template(GuestLanguage::Rust)
        .unwrap()
        .as_ref()
        .unwrap();
    let mut changed_projects = Vec::new();
    for yes in [false, true] {
        for (missing, step) in [(false, "check"), (true, "check"), (true, "build")] {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            let command = if missing {
                "gc03-program-that-does-not-exist"
            } else {
                "cargo build --target wasm32-wasip2"
            };
            std::fs::write(project.path().join("golem.yaml"), format!(
                "manifestVersion: 2.0.0\napp: stale-common\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    templates: rust\n    presets:\n      debug:\n        build:\n          - command: {command}\n"
            )).unwrap();
            std::fs::write(
                project.path().join("Cargo.toml"),
                "[package]\nname = \"stale-common\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            let target = project
                .path()
                .join(Application::on_demand_common_dir_for_language(
                    GuestLanguage::Rust,
                ));
            template
                .generate(
                    &ApplicationName("stale-common".into()),
                    project.path(),
                    &target,
                )
                .unwrap();
            let marker_path = std::fs::read_dir(target.join(".golem-task-results"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let mut marker: TaskResult =
                serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
            marker.cli_version = format!("{}-stale", marker.cli_version);
            std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
            let before = snapshot(project.path());
            for attempt in 0..2 {
                let mut command = cli_command();
                command
                    .current_dir(project.path())
                    .arg("--config-dir")
                    .arg(config.path());
                if yes {
                    command.arg("--yes");
                }
                let output = command.args(["build", "--step", step]).output().unwrap();
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(output.status.success(), !missing, "{text}");
                if missing {
                    assert!(
                        text.contains("Build requires program 'gc03-program-that-does-not-exist'"),
                        "{text}"
                    );
                }
                if snapshot(project.path()) != before {
                    changed_projects.push((yes, missing, step, attempt));
                }
            }
        }
    }
    assert!(
        changed_projects.is_empty(),
        "project changed (yes, missing, step, attempt): {changed_projects:?}"
    );
}

#[test]
fn missing_or_unclassifiable_programs_fail_before_any_project_write() {
    for version in ["1.6.0", "2.0.0"] {
        for command in ["gc03-program-that-does-not-exist", "\"unterminated"] {
            for steps in [
                vec!["--step", "check"],
                vec!["--step", "build"],
                vec!["--step", "check", "--step", "build"],
                vec![],
            ] {
                let project = tempfile::tempdir().unwrap();
                let config = tempfile::tempdir().unwrap();
                let old_output = if version == "1.6.0" {
                    "    outputWasm: obsolete.wasm\n"
                } else {
                    ""
                };
                std::fs::write(project.path().join("golem.yaml"), format!(
                    "manifestVersion: {version}\napp: preflight\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n{old_output}    build:\n      - command: node -e 'throw new Error(\"must not execute\")'\n        mkdirs: [must-not-exist]\n      - command: '{command}'\n"
                )).unwrap();
                let before = snapshot(project.path());
                let output = cli_command()
                    .current_dir(project.path())
                    .arg("--config-dir")
                    .arg(config.path())
                    .args(["--yes", "build"])
                    .args(&steps)
                    .output()
                    .unwrap();
                let stderr = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!output.status.success(), "{version} {steps:?}: {stderr}");
                assert_eq!(
                    snapshot(project.path()),
                    before,
                    "{version} {steps:?}: {stderr}"
                );
                assert!(
                    stderr.contains("Build requires program")
                        || stderr.contains("Unclassifiable build command"),
                    "{stderr}"
                );
            }
        }
    }
}

#[test]
fn current_manifest_checks_show_repairs_without_writing_or_creating_symlinks() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "# $schema: https://schema.golem.cloud/app/golem/1.6.0/golem.schema.json\nmanifestVersion: 2.0.0\napp: current-check\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n").unwrap();
    std::fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
    std::fs::write(project.path().join("component.wasm"), b"prebuilt artifact").unwrap();
    let before = snapshot(project.path());
    for yes in [false, true] {
        let mut command = cli_command();
        command
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path());
        if yes {
            command.arg("--yes");
        }
        let output = command
            .args(["build", "--step", "check", "--step", "check"])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        assert!(text.contains("not applied by check"), "{text}");
        assert_eq!(snapshot(project.path()), before, "{text}");
    }
}

#[test]
fn template_requirements_fail_without_generating_common_files() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "manifestVersion: 2.0.0\napp: template-preflight\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    templates: rust\n    presets:\n      debug:\n        build:\n          - command: gc03-program-that-does-not-exist\n").unwrap();
    let before = snapshot(project.path());
    let output = cli_command()
        .current_dir(project.path())
        .arg("--config-dir")
        .arg(config.path())
        .args(["--yes", "build", "--step", "check"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("Build requires program 'gc03-program-that-does-not-exist'"),
        "{text}"
    );
    assert_eq!(snapshot(project.path()), before, "{text}");
}

#[test]
fn template_checks_inspect_repairs_without_creating_generated_manifests() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "manifestVersion: 2.0.0\napp: template-check\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    templates: rust\n").unwrap();
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"template-check\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    let before = snapshot(project.path());
    let output = cli_command()
        .current_dir(project.path())
        .arg("--config-dir")
        .arg(config.path())
        .args(["--yes", "build", "--step", "check"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    assert!(text.contains("not applied by check"), "{text}");
    assert_eq!(snapshot(project.path()), before, "{text}");
}

#[test]
fn bridge_only_build_without_requests_ignores_unused_programs_and_artifacts() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("golem.yaml"),
        "manifestVersion: 2.0.0\napp: bridge-only\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n    build:\n      - command: gc03-program-that-does-not-exist\n").unwrap();
    let seed = wat::parse_str(include_str!(
        "../../../test-components/size-analysis/retention.wat"
    ))
    .unwrap();
    std::fs::write(project.path().join("component.wasm"), seed).unwrap();
    for present in [true, false] {
        if !present {
            std::fs::remove_file(project.path().join("component.wasm")).unwrap();
        }
        let before = snapshot(project.path());
        let output = cli_command()
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "gen-bridge"])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        assert_eq!(snapshot(project.path()), before, "{text}");
    }
}

#[test]
async fn host_build_preserves_component_exports_and_start_result() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/build-plan/current");
    for name in [
        "golem.yaml",
        "build.mjs",
        "package.json",
        "package-lock.json",
    ] {
        std::fs::copy(fixture.join(name), project.path().join(name)).unwrap();
    }
    let seed = wat::parse_str(include_str!(
        "../../../test-components/size-analysis/retention.wat"
    ))
    .unwrap();
    std::fs::write(project.path().join("seed.wasm"), &seed).unwrap();

    let output = cli_command()
        .current_dir(project.path())
        .arg("--config-dir")
        .arg(config.path())
        .args(["--yes", "build", "--step", "build"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let built = std::fs::read(project.path().join("component.wasm")).unwrap();
    assert_eq!(built, seed);

    let engine = wasmtime::Engine::new(
        &golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache(),
    )
    .unwrap();
    for bytes in [&seed, &built] {
        let component = wasmtime::component::Component::new(&engine, bytes).unwrap();
        let exports = component
            .component_type()
            .exports(&engine)
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(exports, ["run"]);
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_fuel(10_000).unwrap();
        store.set_epoch_deadline(1);
        let instance = wasmtime::component::Linker::new(&engine)
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance
            .get_typed_func::<(), (u32,)>(&mut store, "run")
            .unwrap();
        assert_eq!(run.call_async(&mut store, ()).await.unwrap(), (42,));
    }
}

#[test]
fn incremental_external_build_repairs_stale_and_missing_outputs_after_switching_commands() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(
        root.join("seed.wasm"),
        wat::parse_str(include_str!(
            "../../../test-components/size-analysis/retention.wat"
        ))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("build.cjs"),
        r#"
        const fs = require('node:fs');
        const value = process.argv[2];
        fs.copyFileSync('seed.wasm', 'component.wasm');
        fs.writeFileSync('product.txt', value);
        fs.mkdirSync('required', {recursive:true});
        fs.writeFileSync('required/result.txt', value);
        fs.appendFileSync('trace', value);
    "#,
    )
    .unwrap();
    let set_time = |path: &Path, seconds| {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
            )
            .unwrap();
    };
    for name in ["seed.wasm", "build.cjs"] {
        set_time(&root.join(name), 10);
    }
    let build = |variant: &str| {
        std::fs::write(root.join("golem.yaml"), format!(
            "manifestVersion: 2.0.0\napp: incremental-outputs\nenvironments:\n  local: {{server: local}}\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n    build:\n      - command: node build.cjs {variant}\n        sources: [seed.wasm, build.cjs, optional/**/*.txt]\n        targets: [component.wasm, product.txt, required/*.txt]\n"
        )).unwrap();
        let output = cli_command()
            .current_dir(root)
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "build"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(root.join("component.wasm")).unwrap(),
            std::fs::read(root.join("seed.wasm")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("product.txt")).unwrap(),
            variant
        );
        assert_eq!(
            std::fs::read_to_string(root.join("required/result.txt")).unwrap(),
            variant
        );
    };
    for (variant, trace) in [
        ("A", "A"),
        ("A", "A"),
        ("B", "AB"),
        ("A", "ABA"),
        ("A", "ABA"),
    ] {
        build(variant);
        assert_eq!(std::fs::read_to_string(root.join("trace")).unwrap(), trace);
    }
    std::fs::write(root.join("product.txt"), "stale").unwrap();
    set_time(&root.join("product.txt"), 1);
    set_time(&root.join("component.wasm"), 30);
    set_time(&root.join("required/result.txt"), 30);
    build("A");
    assert_eq!(std::fs::read_to_string(root.join("trace")).unwrap(), "ABAA");
    for (missing, trace) in [("product.txt", "ABAAA"), ("required/result.txt", "ABAAAA")] {
        std::fs::remove_file(root.join(missing)).unwrap();
        build("A");
        assert_eq!(std::fs::read_to_string(root.join("trace")).unwrap(), trace);
        build("A");
        assert_eq!(std::fs::read_to_string(root.join("trace")).unwrap(), trace);
    }
}

#[test]
fn lock_addition_and_removal_invalidate_all_selected_node_commands_without_check_effects() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(
        root.join("seed.wasm"),
        wat::parse_str(include_str!(
            "../../../test-components/size-analysis/retention.wat"
        ))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"lock-presence","version":"1.0.0","private":true}"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".npmrc"),
        "package-lock=false\noffline=true\naudit=false\nfund=false\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("node_modules")).unwrap();
    std::fs::write(
        root.join("build.cjs"),
        r#"
        const fs = require('node:fs');
        const step = process.argv[2];
        if (step === 'B') fs.copyFileSync('seed.wasm', 'component.wasm');
        else fs.writeFileSync('check.txt', 'checked');
        fs.appendFileSync('trace', step);
    "#,
    )
    .unwrap();
    std::fs::write(root.join("golem.yaml"),
        "manifestVersion: 2.0.0\napp: lock-presence\nenvironments:\n  local: {server: local}\ncomponents:\n  app:main:\n    componentWasm: component.wasm\n    build:\n      - command: node build.cjs C\n        sources: [build.cjs, seed.wasm, package.json]\n        targets: [check.txt]\n      - command: node build.cjs B\n        sources: [build.cjs, seed.wasm, package.json]\n        targets: [component.wasm]\n"
    ).unwrap();
    let run = |args: &[&str]| {
        let output = cli_command()
            .current_dir(root)
            .arg("--config-dir")
            .arg(config.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for (phase, present) in [false, true, false].into_iter().enumerate() {
        if present {
            std::fs::write(root.join("package-lock.json"), r#"{"name":"lock-presence","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"lock-presence","version":"1.0.0"}}}"#).unwrap();
        } else if root.join("package-lock.json").exists() {
            std::fs::remove_file(root.join("package-lock.json")).unwrap();
        }
        let before = snapshot(root);
        run(&["--yes", "build", "--step", "check"]);
        assert_eq!(snapshot(root), before);
        for _ in 0..2 {
            run(&["--yes", "build", "--step", "build"]);
            assert_eq!(
                std::fs::read_to_string(root.join("trace")).unwrap(),
                "CB".repeat(phase + 1)
            );
            assert_eq!(
                std::fs::read_to_string(root.join("check.txt")).unwrap(),
                "checked"
            );
            assert_eq!(
                std::fs::read(root.join("component.wasm")).unwrap(),
                std::fs::read(root.join("seed.wasm")).unwrap()
            );
        }
    }
    std::fs::remove_dir_all(root.join("node_modules")).unwrap();
    run(&["clean"]);
    assert!(!root.join("node_modules").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("trace")).unwrap(),
        "CBCBCB"
    );
}

#[test]
fn serial_recipes_share_preparation_and_ignore_manifest_insertion_order() {
    let mut plans = Vec::new();
    for (reverse, fail) in [(false, false), (true, false), (false, true)] {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut components = String::new();
        for name in if reverse { ["b", "a"] } else { ["a", "b"] } {
            std::fs::create_dir(project.path().join(name)).unwrap();
            components.push_str(&format!(
                "  app:{name}:\n    dir: {name}\n    componentWasm: component.wasm\n    build:\n      - command: node record.cjs {name} first\n        dir: ..\n        env:\n          PLAN_DUMP_PRIVATE: not-for-plan-output\n      - command: node record.cjs {name} second\n        dir: ..\n"
            ));
        }
        std::fs::write(project.path().join("golem.yaml"), format!(
            "manifestVersion: 2.0.0\napp: serial-recipes\nenvironments:\n  local:\n    server: local\ncomponents:\n{components}"
        )).unwrap();
        std::fs::write(project.path().join("package.json"),
            r#"{"name":"serial-recipes","version":"0.0.0","private":true,"scripts":{"postinstall":"node record.cjs prepare","record":"node record.cjs"}}"#
        ).unwrap();
        std::fs::write(
            project.path().join(".npmrc"),
            "audit=false\nfund=false\noffline=true\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("record.cjs"),
            r#"
const fs = require('node:fs');
const [name, step] = process.argv.slice(2);
fs.appendFileSync('trace', step ? `${name}:${step}\n` : `${name}\n`);
if (name === 'prepare') fs.mkdirSync('node_modules', {recursive: true});
if (name === 'a' && fs.existsSync('fail')) throw new Error('expected producer failure');
if (step === 'second') fs.copyFileSync('seed.wasm', `${name}/component.wasm`);
"#,
        )
        .unwrap();
        let seed = wat::parse_str(include_str!(
            "../../../test-components/size-analysis/retention.wat"
        ))
        .unwrap();
        std::fs::write(project.path().join("seed.wasm"), &seed).unwrap();
        if fail {
            std::fs::write(project.path().join("fail"), "").unwrap();
        }
        let before = snapshot(project.path());
        let check = cli_command()
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "check"])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
        assert_eq!(snapshot(project.path()), before);
        let check_text = format!(
            "{}{}",
            String::from_utf8_lossy(&check.stdout),
            String::from_utf8_lossy(&check.stderr)
        );
        let start = check_text.find("checked host build stages").unwrap();
        let end = check_text[start..]
            .find("Preflight checks declared requirements")
            .unwrap()
            + start;
        let dump = &check_text[start..end];
        let mut remaining = dump;
        for command in [
            "node record.cjs a first",
            "node record.cjs a second",
            "node record.cjs b first",
            "node record.cjs b second",
        ] {
            let operation = format!("operation: external command {command:?}");
            remaining = remaining
                .split_once(&operation)
                .unwrap_or_else(|| panic!("missing ordered operation {operation} in {dump}"))
                .1;
        }
        assert!(!dump.contains("not-for-plan-output"), "{dump}");
        assert!(!check_text.contains("Executing external command"));
        let output = cli_command()
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "build"])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.success(), !fail, "{text}");
        assert_eq!(
            std::fs::read_to_string(project.path().join("trace")).unwrap(),
            if fail {
                "prepare\na:first\n"
            } else {
                "prepare\na:first\na:second\nb:first\nb:second\n"
            },
            "{text}"
        );
        if fail {
            assert!(text.contains("expected producer failure"), "{text}");
            assert!(!project.path().join("a/component.wasm").exists());
            assert!(!project.path().join("b/component.wasm").exists());
        } else {
            for name in ["a", "b"] {
                assert_eq!(
                    std::fs::read(project.path().join(format!("{name}/component.wasm"))).unwrap(),
                    seed
                );
            }
            let start = text.find("checked host build stages").unwrap();
            let end = text[start..]
                .find("Preflight checks declared requirements")
                .unwrap()
                + start;
            plans.push(text[start..end].replace(project.path().to_str().unwrap(), "<project>"));
        }
    }
    assert_eq!(plans[0], plans[1]);
}

#[test]
fn cold_middleware_bootstrap_uses_real_discovery_and_rejects_late_duplicates() {
    let seeds = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/build-plan/middleware-bootstrap");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdks/rust/golem-rust")
        .canonicalize()
        .unwrap();
    std::fs::create_dir(seeds.path().join("src")).unwrap();
    std::fs::copy(fixture.join("src/lib.rs"), seeds.path().join("src/lib.rs")).unwrap();
    let manifest = std::fs::read_to_string(fixture.join("Cargo.toml")).unwrap();
    std::fs::write(
        seeds.path().join("Cargo.toml"),
        manifest.replace("__GOLEM_RUST_PATH__", sdk.to_str().unwrap()),
    )
    .unwrap();
    for (name, features) in [
        ("r-x", "provider,x"),
        ("r-y", "provider,y"),
        ("r", "provider"),
        ("x", "x"),
        ("y", "y"),
        ("alias-x", "alias-x"),
        ("empty", ""),
    ] {
        let output = Command::new("cargo")
            .current_dir(seeds.path())
            .args(["build", "--target", "wasm32-wasip2", "--features", features])
            .env("CARGO_BUILD_JOBS", "2")
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_TARGET_DIR", seeds.path().join("target"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "seed {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::copy(
            seeds
                .path()
                .join("target/wasm32-wasip2/debug/middleware_bootstrap.wasm"),
            seeds.path().join(format!("{name}.wasm")),
        )
        .unwrap();
    }

    let mut generated_apis = Vec::new();
    for (r, p, q, scope, expected, trace) in [
        ("r-x", "empty", "y", None, "ok", "r\nq\np\n"),
        ("r-y", "x", "empty", None, "ok", "r\np\nq\n"),
        (
            "r-x",
            "x",
            "y",
            None,
            "defined by multiple components",
            "r\nq\np\n",
        ),
        (
            "r-x",
            "alias-x",
            "y",
            None,
            "defined by multiple components",
            "r\nq\np\n",
        ),
        (
            "r",
            "x",
            "y",
            None,
            "unresolved build dependency cycle",
            "r\n",
        ),
        (
            "r",
            "empty",
            "empty",
            Some("app:r"),
            "was not discovered",
            "r\n",
        ),
        (
            "r-x",
            "empty",
            "y",
            Some("app:r"),
            "was not discovered",
            "r\n",
        ),
        (
            "failed-r",
            "empty",
            "y",
            None,
            "expected producer failure",
            "r\n",
        ),
    ] {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("inputs")).unwrap();
        for (name, seed) in [("r", r), ("p", p), ("q", q)] {
            std::fs::create_dir(project.path().join(name)).unwrap();
            let seed = if seed == "failed-r" { "r-x" } else { seed };
            std::fs::copy(
                seeds.path().join(format!("{seed}.wasm")),
                project.path().join(format!("inputs/{name}.wasm")),
            )
            .unwrap();
        }
        if r == "failed-r" {
            std::fs::write(project.path().join("fail"), "").unwrap();
            std::fs::copy(
                project.path().join("inputs/r.wasm"),
                project.path().join("r/component.wasm"),
            )
            .unwrap();
        }
        std::fs::write(
            project.path().join("build.cjs"),
            r#"
const fs = require('node:fs');
const [name, dependency] = process.argv.slice(2);
if (dependency) {
    const root = `../golem-temp/bridge-sdk/rust/internal/${dependency}-tool-guest-client`;
    if (!fs.existsSync(`${root}/Cargo.toml`)) throw new Error(`bridge not ready for ${name}`);
}
fs.appendFileSync('../trace', `${name}\n`);
if (name === 'r' && fs.existsSync('../fail')) throw new Error('expected producer failure');
fs.copyFileSync(`../inputs/${name}.wasm`, 'component.wasm');
"#,
        )
        .unwrap();
        let declaration = scope
            .map(|scope| serde_json::json!({"component": scope}))
            .unwrap_or_else(|| serde_json::json!({}));
        let mut manifest = serde_json::json!({
            "manifestVersion": "2.0.0", "app": "cold-bootstrap",
            "environments": {"local": {"server": "local", "tools": {
                "tool-x": {"filesystemAccess": "allowed", "middleware": [{"name": "layer-x", "filesystemAccess": "allowed"}]},
                "tool-y": {"filesystemAccess": "allowed", "middleware": [{"name": "layer-y", "filesystemAccess": "allowed"}]}
            }}},
            "components": {},
            "tools": {"tool-x": {}, "tool-y": {}, "middleware": {"layer-x": declaration, "layer-y": declaration}}
        });
        for (name, dependency) in [("r", None), ("p", Some("tool-y")), ("q", Some("tool-x"))] {
            let mut component = serde_json::json!({
                "dir": name, "templates": "rust", "buildMergeMode": "replace", "componentWasm": "component.wasm",
                "build": [{"command": format!("node ../build.cjs {name} {}", dependency.unwrap_or_default())}]
            });
            if let Some(dependency) = dependency {
                component["dependencies"] =
                    serde_json::json!({"tools": [format!("app:r/{dependency}")]});
            }
            manifest["components"][format!("app:{name}")] = component;
        }
        std::fs::write(
            project.path().join("golem.yaml"),
            serde_yaml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            !project
                .path()
                .join("golem-temp/extracted-component-metadata")
                .exists()
        );
        let output = cli_command()
            .current_dir(project.path())
            .arg("--config-dir")
            .arg(config.path())
            .args(["--yes", "build", "--step", "build", "--step", "gen-bridge"])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.success(),
            expected == "ok",
            "{r}/{p}/{q} {scope:?}: {text}\nDiscovered metadata: {:?}",
            snapshot(
                &project
                    .path()
                    .join("golem-temp/extracted-component-metadata")
            )
            .into_iter()
            .filter(|(path, _)| path
                .extension()
                .is_some_and(|extension| extension == "json"))
            .map(|(path, bytes)| (path, String::from_utf8_lossy(&bytes).into_owned()))
            .collect::<Vec<_>>()
        );
        if expected != "ok" {
            assert!(text.contains(expected), "{r}/{p}/{q} {scope:?}: {text}");
        }
        assert_eq!(
            std::fs::read_to_string(project.path().join("trace")).unwrap(),
            trace,
            "{text}"
        );
        for name in trace.lines() {
            assert_eq!(
                std::fs::read(project.path().join(format!("{name}/component.wasm"))).unwrap(),
                std::fs::read(project.path().join(format!("inputs/{name}.wasm"))).unwrap()
            );
        }
        if r == "failed-r" {
            assert!(!project.path().join("p/component.wasm").exists());
            assert!(!project.path().join("q/component.wasm").exists());
            assert!(!project.path().join("golem-temp/bridge-sdk").exists());
            assert!(
                !project
                    .path()
                    .join("golem-temp/extracted-component-metadata")
                    .exists()
            );
        }
        if expected == "ok" {
            generated_apis.push(["tool-x", "tool-y"].map(|name| {
                std::fs::read(project.path().join(format!(
                    "golem-temp/bridge-sdk/rust/internal/{name}-tool-guest-client/src/lib.rs"
                )))
                .unwrap()
            }));
        }
    }
    assert_eq!(
        generated_apis[0], generated_apis[1],
        "transparent middleware placement must not change the generated APIs"
    );
}

#[test]
fn check_rejects_duplicate_manifest_sources_without_filename_precedence_or_writes() {
    for included in ["a.yaml", "z.yaml"] {
        for (fragment, duplicate) in [
            ("app: shadow\n", true),
            ("environments:\n  local:\n    server: cloud\n", true),
            ("environments:\n  remote:\n    server: cloud\n", false),
        ] {
            let project = tempfile::tempdir().unwrap();
            let config = tempfile::tempdir().unwrap();
            std::fs::write(project.path().join("golem.yaml"), format!(
                "manifestVersion: {}\napp: primary\nincludes: [{included}]\nenvironments:\n  local:\n    server: local\ncomponents:\n  app:main:\n    componentWasm: seed.wasm\n",
                golem_cli::versions::sdk::MANIFEST
            )).unwrap();
            std::fs::write(
                project.path().join(included),
                format!(
                    "manifestVersion: {}\n{fragment}",
                    golem_cli::versions::sdk::MANIFEST
                ),
            )
            .unwrap();
            std::fs::write(
                project.path().join("seed.wasm"),
                wat::parse_str("(component)").unwrap(),
            )
            .unwrap();
            let before = snapshot(project.path());
            let output = cli_command()
                .current_dir(project.path())
                .arg("--config-dir")
                .arg(config.path())
                .args(["--yes", "build", "--step", "check"])
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.status.success(), !duplicate, "{fragment}: {text}");
            if duplicate {
                for expected in ["multiple sources", "golem.yaml", included] {
                    assert!(text.contains(expected), "{text}");
                }
            }
            assert_eq!(snapshot(project.path()), before, "{text}");
        }
    }
}

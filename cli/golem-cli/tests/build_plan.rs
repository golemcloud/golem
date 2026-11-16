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

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_golem-cli"));
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
            "check loads the proposed conversion without applying it: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            snapshot(project.path()),
            before,
            "check must not upgrade or repair a project, even with --yes; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Applying"));

        let build = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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
                let output = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_golem-cli"));
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
    let output = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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
fn bridge_only_build_checks_the_artifact_not_unused_build_programs() {
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
        let output = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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
        assert_eq!(output.status.success(), present, "{text}");
        if !present {
            assert!(text.contains("Build requires existing file"), "{text}");
            assert_eq!(snapshot(project.path()), before, "{text}");
        }
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

    let output = Command::new(env!("CARGO_BIN_EXE_golem-cli"))
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

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
            !output.status.success(),
            "an old manifest needs explicit upgrade"
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

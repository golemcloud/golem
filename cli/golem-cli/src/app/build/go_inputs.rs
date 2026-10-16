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

//! What `go mod tidy` resolves a module's requirements from, as one hash.
//!
//! Tidy reads the imports of every package the module and its tests could
//! build, under any build constraint, and the modules that local `replace`
//! directives point to. Tracking exactly that set means following the go
//! command's package rules, so the hash is deliberately conservative instead:
//! it covers the name and content of every `.go`, `go.mod` and `go.sum` file
//! in the module and in each local replacement, transitively. Any change to
//! Go sources reruns tidy; nothing tidy would see escapes it.

use crate::app::edit::go_mod;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

/// The hash of the tidy inputs of the Go module at `module_root`.
pub fn tidy_inputs_hash(module_root: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut seen = BTreeSet::new();
    let mut roots = vec![module_root.to_path_buf()];
    while let Some(root) = roots.pop() {
        let key = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !seen.insert(key) || !root.is_dir() {
            continue;
        }
        hasher.update(root.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        for (relative, content) in module_files(&root) {
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hasher.update(blake3::hash(&content).as_bytes());
        }
        if let Ok(go_mod) = std::fs::read_to_string(root.join("go.mod")) {
            roots.extend(
                go_mod::local_replacements(&go_mod)
                    .into_iter()
                    .map(|target| root.join(target)),
            );
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// The `.go`, `go.mod` and `go.sum` files of the module at `root`, by path
/// relative to it, in a stable order. Symlinks are followed; one that is
/// broken or loops is skipped, since it cannot be part of the build. `vendor`,
/// `.git` and nested modules are left out.
fn module_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let walker = WalkDir::new(root)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !(name == "vendor" || name == ".git" || entry.path().join("go.mod").exists())
        });
    let mut files = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                debug!("skipping a path the Go tidy inputs cannot read: {err}");
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy();
        if !entry.file_type().is_file()
            || !(name.ends_with(".go") || name == "go.mod" || name == "go.sum")
        {
            continue;
        }
        match std::fs::read(entry.path()) {
            Ok(content) => {
                let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
                files.push((relative.to_path_buf(), content));
            }
            Err(err) => debug!("skipping {}: {err}", entry.path().display()),
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::tidy_inputs_hash;
    use std::path::{Path, PathBuf};
    use test_r::test;

    fn write(path: PathBuf, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn changes(root: &Path, seen: &mut String, what: &str) {
        let now = tidy_inputs_hash(root);
        assert_ne!(&now, seen, "{what} did not change the tidy inputs");
        *seen = now;
    }

    /// Every change the review found tidy reading — in a local replacement,
    /// in a `_` directory or file, through a symlink, in a build constraint or
    /// an escaped import, and in a replacement's own go.mod — changes the hash.
    #[test]
    fn dependency_relevant_changes_change_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let lib = dir.path().join("my lib");
        write(
            root.join("go.mod"),
            "module app\n\nrequire lib v0.0.0\n\nreplace lib => \"../my lib\"\n",
        );
        write(root.join("main.go"), "package main\n");
        write(lib.join("go.mod"), "module lib\n");
        let mut seen = tidy_inputs_hash(&root);

        write(
            lib.join("lib.go"),
            "package lib\n\nimport \"example.com/a\"\n",
        );
        changes(&root, &mut seen, "an import in a quoted local replacement");
        write(
            lib.join("go.mod"),
            "module lib\n\nrequire example.com/a v1.2.0\n",
        );
        changes(&root, &mut seen, "a replacement's go.mod");
        write(root.join("_private/p.go"), "package p\n");
        changes(&root, &mut seen, "a file in a _ directory");
        std::fs::rename(root.join("_private/p.go"), root.join("_private/active.go")).unwrap();
        changes(&root, &mut seen, "a renamed file");
        write(root.join("tool.go"), "// +build ignore\n\npackage main\n");
        changes(&root, &mut seen, "a new file");
        write(root.join("tool.go"), "package main\n");
        changes(&root, &mut seen, "a removed legacy build constraint");
        #[cfg(unix)]
        {
            write(dir.path().join("outside/linked.go"), "package main\n");
            std::os::unix::fs::symlink(
                dir.path().join("outside/linked.go"),
                root.join("linked.go"),
            )
            .unwrap();
            changes(&root, &mut seen, "a symlinked file");
        }
    }

    /// Links that cannot be part of the build — a dangling one, and a
    /// directory link back to the module — do not stop the hash.
    #[cfg(unix)]
    #[test]
    fn unrelated_broken_links_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        write(root.join("go.mod"), "module app\n");
        write(root.join("main.go"), "package main\n");
        let before = tidy_inputs_hash(&root);
        std::os::unix::fs::symlink(root.join("missing"), root.join("missing-link")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        assert_eq!(tidy_inputs_hash(&root), before);
    }

    /// Editing a non-Go file does not rerun tidy.
    #[test]
    fn other_files_do_not_change_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        write(root.join("go.mod"), "module app\n");
        write(root.join("main.go"), "package main\n");
        let before = tidy_inputs_hash(&root);
        write(root.join("README.md"), "notes\n");
        assert_eq!(tidy_inputs_hash(&root), before);
    }
}

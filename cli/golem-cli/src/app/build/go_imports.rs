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

//! The import paths of a Go module that `go mod tidy` resolves to required
//! modules: every import outside the standard library, from every package of
//! the module and of the modules its local replacements point to.

use crate::app::edit::go_mod;
use crate::app::edit::main_go::import_paths;
use std::collections::BTreeSet;
use std::path::Path;
use walkdir::WalkDir;

/// What `go mod tidy` resolves requirements from, for the Go module at
/// `module_root`: every non-standard-library import, each with the build
/// constraint of the file that declares it. The scan is deliberately wider than
/// the go command's package patterns, so that no change tidy would see escapes
/// it: it reads directories starting with `_` or `.` (an explicit import
/// reaches them), follows symlinks, reads files whatever their build
/// constraints, and also reads the modules a local `replace` points to. It
/// skips only `vendor`, `.git` and nested modules.
pub fn module_imports(module_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut imports = BTreeSet::new();
    let mut roots = vec![module_root.to_path_buf()];
    if let Ok(go_mod) = std::fs::read_to_string(module_root.join("go.mod")) {
        roots.extend(
            go_mod::local_replacements(&go_mod)
                .into_iter()
                .map(|target| module_root.join(target)),
        );
    }
    for root in roots {
        if root.is_dir() {
            scan(&root, &mut imports)?;
        }
    }
    Ok(imports)
}

fn scan(root: &Path, imports: &mut BTreeSet<String>) -> anyhow::Result<()> {
    let walker = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !(name == "vendor" || name == ".git" || entry.path().join("go.mod").exists())
        });
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() || !entry.file_name().to_string_lossy().ends_with(".go") {
            continue;
        }
        let source = std::fs::read_to_string(entry.path())?;
        let constraint = build_constraint(&source);
        imports.extend(
            import_paths(&source)
                .into_iter()
                .filter(|path| is_third_party(path))
                .map(|path| format!("{path}\t{constraint}")),
        );
    }
    Ok(())
}

/// The `//go:build` line of a Go file, from the comments before its package
/// clause, or empty.
fn build_constraint(source: &str) -> &str {
    source
        .lines()
        .map(str::trim)
        .take_while(|line| !line.starts_with("package "))
        .find(|line| line.starts_with("//go:build"))
        .unwrap_or_default()
}

/// A standard-library path has no dot in its first element.
fn is_third_party(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|first| first.contains('.'))
}

#[cfg(test)]
mod tests {
    use super::{is_third_party, module_imports};
    use crate::app::edit::main_go::import_paths as imports_of;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn reads_single_grouped_named_and_commented_imports() {
        let source = r#"// Package doc, mentioning import "nope".
package main // trailing

/* import "also-nope" */
import "fmt"
import alias "github.com/a/b"
import (
	"net/http"
	_ "example.com/side"
	. `example.org/dot`
	// "example.net/commented"
)

import "after/group"

func main() { _ = "github.com/not/an/import" }
"#;
        assert_eq!(
            imports_of(source),
            vec![
                "fmt",
                "github.com/a/b",
                "net/http",
                "example.com/side",
                "example.org/dot",
                "after/group",
            ]
        );
    }

    #[test]
    fn a_file_without_imports_has_none() {
        assert!(imports_of("package x\n\nfunc F() {}\n").is_empty());
        assert!(imports_of("not go").is_empty());
    }

    #[test]
    fn third_party_paths_have_a_dot_in_the_first_element() {
        assert!(is_third_party("github.com/a/b"));
        assert!(!is_third_party("net/http"));
        assert!(!is_third_party("agent-sdk-go/agents/counter"));
    }

    /// Each change vigoo's review found escaping the tidy marker changes the
    /// scanned set: a new import in a local replacement, in a `_` directory,
    /// through a symlinked file, a removed `ignore` constraint, and an escaped
    /// import path.
    #[test]
    fn every_dependency_relevant_change_changes_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let lib = dir.path().join("lib");
        let write = |path: std::path::PathBuf, content: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            root.join("go.mod"),
            "module app\n\nrequire lib v0.0.0\n\nreplace lib => ../lib\n",
        );
        write(root.join("main.go"), "package main\n\nimport \"fmt\"\n");
        write(lib.join("go.mod"), "module lib\n");
        let mut seen = module_imports(&root).unwrap();
        assert!(seen.is_empty(), "{seen:?}");
        let changed = |seen: &mut BTreeSet<String>, what: &str| {
            let now = module_imports(&root).unwrap();
            assert_ne!(&now, seen, "{what} did not change the scan");
            *seen = now;
        };

        write(
            lib.join("lib.go"),
            "package lib\n\nimport \"example.com/a\"\n",
        );
        changed(&mut seen, "an import in a local replacement");
        write(
            root.join("_private/p.go"),
            "package p\n\nimport \"example.com/b\"\n",
        );
        changed(&mut seen, "an import in a _ directory");
        #[cfg(unix)]
        {
            write(
                dir.path().join("outside/linked.go"),
                "package main\n\nimport \"example.com/c\"\n",
            );
            std::os::unix::fs::symlink(
                dir.path().join("outside/linked.go"),
                root.join("linked.go"),
            )
            .unwrap();
            changed(&mut seen, "an import through a symlinked file");
        }
        write(
            root.join("tool.go"),
            "//go:build ignore\n\npackage main\n\nimport \"example.com/d\"\n",
        );
        seen = module_imports(&root).unwrap();
        write(
            root.join("tool.go"),
            "package main\n\nimport \"example.com/d\"\n",
        );
        changed(&mut seen, "a removed ignore constraint");
        write(
            root.join("quote.go"),
            "package main\n\nimport \"rsc\\x2eio/quote\"\n",
        );
        changed(&mut seen, "an escaped import path");
        assert!(
            seen.iter().any(|entry| entry.starts_with("rsc.io/quote\t")),
            "{seen:?}"
        );
    }
}

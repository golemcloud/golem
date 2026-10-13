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
//! the module.

use crate::app::edit::main_go::import_paths;
use std::collections::BTreeSet;
use std::path::Path;
use walkdir::WalkDir;

/// The non-standard-library import paths of the Go module at `module_root`. It
/// skips what the go command ignores: `vendor` and `testdata` directories,
/// names starting with `.` or `_`, and nested modules.
pub fn module_imports(module_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut imports = BTreeSet::new();
    let walker = WalkDir::new(module_root).into_iter().filter_entry(|entry| {
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        !(name.starts_with('.')
            || name.starts_with('_')
            || name == "vendor"
            || name == "testdata"
            || entry.path().join("go.mod").exists())
    });
    for entry in walker {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy();
        if !entry.file_type().is_file()
            || !name.ends_with(".go")
            || name.starts_with('.')
            || name.starts_with('_')
        {
            continue;
        }
        let source = std::fs::read_to_string(entry.path())?;
        imports.extend(
            import_paths(&source)
                .into_iter()
                .filter(|path| is_third_party(path)),
        );
    }
    Ok(imports)
}

/// A standard-library path has no dot in its first element.
fn is_third_party(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|first| first.contains('.'))
}

#[cfg(test)]
mod tests {
    use super::is_third_party;
    use crate::app::edit::main_go::import_paths as imports_of;
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
}

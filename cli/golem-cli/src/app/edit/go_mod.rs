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

//! Minimal, formatting-tolerant `go.mod` editing for reconciling the Golem SDK
//! dependency with the active SDK overrides on every build (mirrors the Rust
//! `Cargo.toml` and TS `package.json` fix steps).
//!
//! Only two things are touched, both anchored on the SDK module path:
//!   - the `require` version for the SDK module (single-line or block form);
//!   - a top-level `replace <module> => <path>` directive — present when a local
//!     SDK path override is active, absent otherwise.
//!
//! `go mod tidy` reformats `go.mod` (block requires, tabs), so matching is by
//! module path and structure, never exact text.

/// Reconcile the SDK dependency in `content`:
///   - set the SDK module's `require` version to `version`;
///   - if `replace_path` is `Some`, ensure `replace <module> => <path>`;
///   - if `None`, remove any such replace directive.
///
/// Returns the updated `go.mod` text. A trailing newline is preserved/ensured.
pub fn reconcile_sdk_dependency(
    content: &str,
    module: &str,
    version: &str,
    replace_path: Option<&str>,
) -> String {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    set_require_version(&mut lines, module, version);
    set_replace_directive(&mut lines, module, replace_path);

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A `require` entry for `module` — either `require <module> <ver>` (single) or
/// `<module> <ver>` inside a `require (…)` block — has its version set. A require
/// line has no `=>` (which distinguishes it from a `replace` line that also
/// names the module).
fn set_require_version(lines: &mut [String], module: &str, version: &str) {
    let mut in_require_block = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim();

        if !in_require_block && trimmed.starts_with("require (") {
            in_require_block = true;
            continue;
        }
        if in_require_block && trimmed == ")" {
            in_require_block = false;
            continue;
        }

        if trimmed.contains("=>") {
            continue; // replace line, not require
        }

        // Single-line: `require <module> <ver>`
        if let Some(rest) = trimmed.strip_prefix("require ") {
            if first_token(rest) == Some(module) {
                let indent = leading_ws(line);
                let suffix = trailing_comment(rest);
                *line = format!("{indent}require {module} {version}{suffix}");
            }
            continue;
        }

        // Block entry: `<module> <ver>`
        if in_require_block && first_token(trimmed) == Some(module) {
            let indent = leading_ws(line);
            let suffix = trailing_comment(trimmed);
            *line = format!("{indent}{module} {version}{suffix}");
        }
    }
}

/// Ensure or remove a top-level `replace <module> => <path>` directive.
/// Replace *blocks* are also handled: an entry `<module> => <path>` inside a
/// `replace (…)` block is updated or removed.
fn set_replace_directive(lines: &mut Vec<String>, module: &str, replace_path: Option<&str>) {
    let mut updated = false;
    let mut in_replace_block = false;
    let mut removals = Vec::new();

    for (idx, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();

        if !in_replace_block && trimmed.starts_with("replace (") {
            in_replace_block = true;
            continue;
        }
        if in_replace_block && trimmed == ")" {
            in_replace_block = false;
            continue;
        }

        let is_single = trimmed
            .strip_prefix("replace ")
            .map(|rest| replace_lhs_is(rest, module))
            .unwrap_or(false);
        let is_block_entry = in_replace_block && replace_lhs_is(trimmed, module);

        if is_single || is_block_entry {
            match replace_path {
                Some(path) if is_single => {
                    *line = format!("replace {module} => {path}");
                    updated = true;
                }
                Some(path) => {
                    let indent = leading_ws(line);
                    *line = format!("{indent}{module} => {path}");
                    updated = true;
                }
                None => removals.push(idx),
            }
        }
    }

    for idx in removals.into_iter().rev() {
        lines.remove(idx);
    }

    // Append a fresh directive if wanted but none existed.
    if let Some(path) = replace_path {
        if !updated {
            if !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            lines.push(format!("replace {module} => {path}"));
        }
    }
}

/// True if the text before `=>` names exactly `module` (ignoring an optional
/// version token, e.g. `replace mod v1 => …`).
fn replace_lhs_is(rest: &str, module: &str) -> bool {
    match rest.split("=>").next() {
        Some(lhs) => lhs.split_whitespace().next() == Some(module),
        None => false,
    }
}

fn first_token(s: &str) -> Option<&str> {
    s.split_whitespace().next()
}

fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// The trailing `// …` comment of a require entry (e.g. ` // indirect`),
/// including its leading space, or empty.
fn trailing_comment(entry: &str) -> String {
    match entry.find("//") {
        Some(pos) => format!(" {}", entry[pos..].trim_end()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    const MOD: &str = "github.com/golemcloud/golem/sdks/go/golem";

    #[test]
    fn sets_version_in_a_block_and_adds_replace() {
        let src = format!(
            "module app\n\ngo 1.25.5\n\nrequire (\n\t{MOD} v0.0.0\n\tgithub.com/bytecodealliance/componentize-go v0.4.0\n)\n\ntool github.com/bytecodealliance/componentize-go\n"
        );
        let out = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/abs/sdks/go/golem"));
        assert!(out.contains(&format!("\t{MOD} v0.0.0\n")));
        assert!(out.contains(&format!("replace {MOD} => /abs/sdks/go/golem")));
        // componentize-go untouched
        assert!(out.contains("github.com/bytecodealliance/componentize-go v0.4.0"));
    }

    #[test]
    fn updates_version_single_line() {
        let src = format!("module app\n\nrequire {MOD} v0.0.0\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.1.0", None);
        assert!(out.contains(&format!("require {MOD} v0.1.0\n")));
    }

    #[test]
    fn removes_replace_when_switching_to_version() {
        let src =
            format!("module app\n\nrequire {MOD} v0.0.0\n\nreplace {MOD} => /abs/sdks/go/golem\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.1.0", None);
        assert!(!out.contains("replace"));
        assert!(out.contains(&format!("require {MOD} v0.1.0")));
    }

    #[test]
    fn updates_existing_replace_path() {
        let src = format!("module app\n\nrequire {MOD} v0.0.0\n\nreplace {MOD} => /old/path\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/new/path"));
        assert!(out.contains(&format!("replace {MOD} => /new/path")));
        assert!(!out.contains("/old/path"));
    }

    #[test]
    fn preserves_indirect_comment_on_block_entries() {
        let src = format!(
            "module app\n\nrequire (\n\t{MOD} v0.0.0\n\tgolang.org/x/sys v0.37.0 // indirect\n)\n"
        );
        let out = reconcile_sdk_dependency(&src, MOD, "v0.2.0", None);
        assert!(out.contains("golang.org/x/sys v0.37.0 // indirect"));
        assert!(out.contains(&format!("{MOD} v0.2.0")));
    }

    #[test]
    fn idempotent_when_already_reconciled() {
        let src = format!("module app\n\nrequire {MOD} v0.0.0\n\nreplace {MOD} => /p\n");
        let once = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/p"));
        let twice = reconcile_sdk_dependency(&once, MOD, "v0.0.0", Some("/p"));
        assert_eq!(once, twice);
    }
}

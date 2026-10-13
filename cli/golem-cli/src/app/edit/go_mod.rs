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

use std::collections::{BTreeMap, BTreeSet};

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
    let replace_target = replace_path.map(replace_path_token);
    set_replace_directive(&mut lines, module, replace_target.as_deref());

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A filesystem path as the target of a `replace` directive. Go reads a bare
/// token up to whitespace or a comment marker, so a path holding a space, a
/// quote, a comment marker or a control character is written as a Go string
/// literal, as `modfile.AutoQuote` does.
pub fn replace_path_token(path: &str) -> String {
    let must_quote = path.is_empty()
        || path.contains("//")
        || path.contains("/*")
        || path.chars().any(|ch| match ch {
            ' ' | '"' | '\'' | '`' => true,
            '(' | ')' | '[' | ']' | '{' | '}' | ',' => path.chars().count() > 1,
            _ => ch.is_control() || ch.is_whitespace(),
        });
    if must_quote {
        crate::bridge_gen::go::go::go_string(path)
    } else {
        path.to_string()
    }
}

/// Ensure `replace <module> => <target>` when `target` is `Some`, and remove any
/// replace of `module` when it is `None`; nothing else is touched.
pub fn reconcile_replace(content: &str, module: &str, target: Option<&str>) -> String {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    set_replace_directive(&mut lines, module, target);
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Checks the structure of a `go.mod`: one `module` directive, blocks that
/// open and close, and entries of the shape their directive takes (a module and
/// a version for `require` and `exclude`, one path for `tool`, an `=>` for
/// `replace`). It does not check versions or paths.
pub fn validate(content: &str) -> anyhow::Result<()> {
    let mut modules = 0;
    let mut block: Option<String> = None;
    for (number, tokens) in content.lines().map(tokens).enumerate() {
        let line = number + 1;
        if tokens.is_empty() {
            continue;
        }
        if let Some(kind) = &block {
            if tokens == [")"] {
                block = None;
            } else {
                check_entry(kind, &tokens).map_err(|e| anyhow::anyhow!("line {line}: {e}"))?;
            }
            continue;
        }
        let kind = tokens[0].as_str();
        let rest = &tokens[1..];
        match kind {
            "module" => {
                modules += 1;
                if rest.len() != 1 {
                    anyhow::bail!("line {line}: module expects one path");
                }
            }
            "go" | "toolchain" | "godebug" if !rest.is_empty() => {}
            "require" | "exclude" | "replace" | "retract" | "tool" | "ignore" | "godebug"
                if rest == ["("] =>
            {
                block = Some(kind.to_string());
            }
            "require" | "exclude" | "replace" | "retract" | "tool" | "ignore" => {
                check_entry(kind, rest).map_err(|e| anyhow::anyhow!("line {line}: {e}"))?;
            }
            _ => anyhow::bail!("line {line}: unknown directive {kind}"),
        }
    }
    if block.is_some() {
        anyhow::bail!("a block is not closed");
    }
    if modules != 1 {
        anyhow::bail!("expected one module directive, found {modules}");
    }
    Ok(())
}

fn check_entry(kind: &str, tokens: &[String]) -> anyhow::Result<()> {
    let ok = match kind {
        "require" | "exclude" => tokens.len() == 2,
        "tool" | "ignore" => tokens.len() == 1,
        "replace" => tokens.iter().any(|t| t == "=>"),
        _ => !tokens.is_empty(),
    };
    if !ok {
        anyhow::bail!("malformed {kind} entry");
    }
    Ok(())
}

/// The tokens of one `go.mod` line, as the go command reads them: separated by
/// whitespace, with `//` starting a comment outside a quoted token, and with
/// quoted tokens (`"…"` or `` `…` ``) kept whole, quotes included.
fn tokens(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let comment = |i: usize| bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/');
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
        } else if comment(i) {
            break;
        } else if b == b'"' || b == b'`' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b {
                if b == b'"' && bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            out.push(line[start..i].to_string());
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !comment(i) {
                i += 1;
            }
            out.push(line[start..i].to_string());
        }
    }
    out
}

/// Merge the dependencies `update` declares into `current`: each `require`,
/// `replace` and `tool` entry of `update` whose module `current` does not
/// name yet is added. An entry `current` already has keeps its version, since
/// `go mod tidy` or the user may have moved it on; the next build's tidy makes
/// the result consistent.
pub fn merge_documents(current: &str, update: &str) -> anyhow::Result<String> {
    let mut lines: Vec<String> = current.lines().map(|l| l.to_string()).collect();
    let present = directive_entries(current);
    for (kind, entries) in directive_entries(update) {
        let known = present.get(&kind);
        let missing: Vec<(String, String)> = entries
            .into_iter()
            .filter(|(key, _)| !known.is_some_and(|k| k.iter().any(|(have, _)| have == key)))
            .collect();
        if missing.is_empty() {
            continue;
        }
        match block_end(&lines, &kind) {
            Some(end) => {
                for (offset, (_, entry)) in missing.into_iter().enumerate() {
                    lines.insert(end + offset, format!("\t{entry}"));
                }
            }
            None => {
                if !lines.last().is_none_or(|l| l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("{kind} ("));
                lines.extend(missing.into_iter().map(|(_, entry)| format!("\t{entry}")));
                lines.push(")".to_string());
            }
        }
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    validate(&out)?;
    Ok(out)
}

/// Per directive kind (`require`, `replace`, `tool`): its entries as (the
/// module the entry is about, the entry's tokens joined by a space), from
/// both the single-line and the block form.
fn directive_entries(content: &str) -> BTreeMap<String, Vec<(String, String)>> {
    const KINDS: [&str; 3] = ["require", "replace", "tool"];
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut block: Option<String> = None;
    for tokens in content.lines().map(tokens) {
        if tokens.is_empty() {
            continue;
        }
        if let Some(kind) = &block {
            if tokens == [")"] {
                block = None;
            } else if KINDS.contains(&kind.as_str()) {
                out.entry(kind.clone())
                    .or_default()
                    .push((tokens[0].clone(), tokens.join(" ")));
            }
            continue;
        }
        if tokens.len() == 2 && tokens[1] == "(" {
            block = Some(tokens[0].clone());
        } else if KINDS.contains(&tokens[0].as_str()) && tokens.len() > 1 {
            out.entry(tokens[0].clone())
                .or_default()
                .push((tokens[1].clone(), tokens[1..].join(" ")));
        }
    }
    out
}

/// The index of the line that closes the first `<kind> (` block, if any.
fn block_end(lines: &[String], kind: &str) -> Option<usize> {
    let start = lines.iter().position(|l| tokens(l) == [kind, "("])?;
    lines[start + 1..]
        .iter()
        .position(|l| tokens(l) == [")"])
        .map(|offset| start + 1 + offset)
}

/// The local directories `replace` directives point to: targets that are a
/// relative (`./`, `../`) or absolute path rather than a module path.
pub fn local_replacements(content: &str) -> Vec<String> {
    directive_entries(content)
        .remove("replace")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, entry)| {
            let target = entry.split(" => ").nth(1)?.split(' ').next()?;
            let target = target.trim_matches(|c| c == '"' || c == '`');
            let local = target.starts_with("./")
                || target.starts_with("../")
                || std::path::Path::new(target).is_absolute();
            local.then(|| target.to_string())
        })
        .collect()
}

/// Merge two `go.sum` files: the union of their lines, sorted as the go
/// command writes them.
pub fn merge_go_sum(current: &str, update: &str) -> anyhow::Result<String> {
    let lines: BTreeSet<&str> = current
        .lines()
        .chain(update.lines())
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let mut out = lines.into_iter().collect::<Vec<_>>().join("\n");
    out.push('\n');
    Ok(out)
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
    if let Some(path) = replace_path
        && !updated
    {
        if !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push(format!("replace {module} => {path}"));
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
    const CORE: &str = "github.com/golemcloud/golem/sdks/go/core";

    /// The guest SDK depends on the shared core module, and a `replace` in a
    /// dependency's go.mod is ignored — so a component that resolves the SDK
    /// from a checkout has to name core itself. The two are reconciled by
    /// separate calls, which must compose.
    #[test]
    fn reconciles_the_sdk_and_core_together() {
        let src =
            format!("module app\n\ngo 1.27.1\n\nrequire (\n\t{MOD} v0.0.0\n\t{CORE} v0.0.0\n)\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/abs/sdks/go/golem"));
        let out = reconcile_sdk_dependency(&out, CORE, "v0.0.0", Some("/abs/sdks/go/core"));

        assert!(out.contains(&format!("replace {MOD} => /abs/sdks/go/golem")));
        assert!(out.contains(&format!("replace {CORE} => /abs/sdks/go/core")));

        // Reconciling again changes nothing: a build must not report a go.mod
        // change every time.
        let again = reconcile_sdk_dependency(&out, MOD, "v0.0.0", Some("/abs/sdks/go/golem"));
        let again = reconcile_sdk_dependency(&again, CORE, "v0.0.0", Some("/abs/sdks/go/core"));
        assert_eq!(again, out);
    }

    /// Switching back to proxy resolution has to drop both replaces, or the
    /// build keeps pointing at a checkout that may not exist.
    #[test]
    fn switching_to_versions_removes_both_replaces() {
        let src = format!(
            "module app\n\nrequire (\n\t{MOD} v0.0.0\n\t{CORE} v0.0.0\n)\n\nreplace {MOD} => /abs/a\n\nreplace {CORE} => /abs/b\n"
        );
        let out = reconcile_sdk_dependency(&src, MOD, "v0.1.0", None);
        let out = reconcile_sdk_dependency(&out, CORE, "v0.1.0", None);

        assert!(!out.contains("replace"));
        assert!(out.contains(&format!("\t{MOD} v0.1.0")));
        assert!(out.contains(&format!("\t{CORE} v0.1.0")));
    }

    #[test]
    fn sets_version_in_a_block_and_adds_replace() {
        let src = format!(
            "module app\n\ngo 1.27.1\n\nrequire (\n\t{MOD} v0.0.0\n\tgithub.com/bytecodealliance/componentize-go v0.4.3\n)\n\ntool github.com/bytecodealliance/componentize-go\n"
        );
        let out = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/abs/sdks/go/golem"));
        assert!(out.contains(&format!("\t{MOD} v0.0.0\n")));
        assert!(out.contains(&format!("replace {MOD} => /abs/sdks/go/golem")));
        // componentize-go untouched
        assert!(out.contains("github.com/bytecodealliance/componentize-go v0.4.3"));
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
    fn quotes_a_replace_path_that_go_would_split() {
        let src = format!("module app\n\nrequire {MOD} v0.0.0\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/home/me/my work/golem"));
        assert!(out.contains(&format!(r#"replace {MOD} => "/home/me/my work/golem""#)));
        assert_eq!(
            reconcile_sdk_dependency(&out, MOD, "v0.0.0", Some("/home/me/my work/golem")),
            out
        );
        assert_eq!(replace_path_token("/plain/path"), "/plain/path");
        assert_eq!(replace_path_token(r"C:\src\golem"), r"C:\src\golem");
        assert_eq!(
            replace_path_token(r"C:\My Files\golem"),
            r#""C:\\My Files\\golem""#
        );
        assert_eq!(replace_path_token("/a//b"), r#""/a//b""#);
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
    fn reconcile_replace_adds_updates_and_is_idempotent() {
        let fork = "github.com/golemcloud/go-pkg v0.2.3-golem.1";
        let added = reconcile_replace("module m\n", "go.bytecodealliance.org/pkg", Some(fork));
        assert_eq!(
            added,
            "module m\n\nreplace go.bytecodealliance.org/pkg => github.com/golemcloud/go-pkg v0.2.3-golem.1\n"
        );
        assert_eq!(
            reconcile_replace(&added, "go.bytecodealliance.org/pkg", Some(fork)),
            added
        );
        let older = added.replace("golem.1", "golem.0");
        assert_eq!(
            reconcile_replace(&older, "go.bytecodealliance.org/pkg", Some(fork)),
            added
        );
    }

    #[test]
    fn idempotent_when_already_reconciled() {
        let src = format!("module app\n\nrequire {MOD} v0.0.0\n\nreplace {MOD} => /p\n");
        let once = reconcile_sdk_dependency(&src, MOD, "v0.0.0", Some("/p"));
        let twice = reconcile_sdk_dependency(&once, MOD, "v0.0.0", Some("/p"));
        assert_eq!(once, twice);
    }

    /// A template added after the first build meets a go.mod that tidy has
    /// rewritten: the template's own requires, replaces and tools are added,
    /// and what the component already has keeps its version.
    #[test]
    fn merge_adds_what_the_update_declares_and_keeps_current_versions() {
        let current = format!(
            "module app\n\ngo 1.27.1\n\nrequire (\n\t{MOD} v0.1.0\n\tgolang.org/x/sys v0.37.0 // indirect\n)\n\ntool github.com/bytecodealliance/componentize-go\n"
        );
        let update = format!(
            "module app\n\ngo 1.27.1\n\nrequire (\n\t{MOD} v0.0.0\n\t{CORE} v0.0.0\n)\n\nreplace {MOD} => /abs/golem\n\ntool github.com/bytecodealliance/componentize-go\n"
        );
        let merged = merge_documents(&current, &update).unwrap();
        assert!(merged.contains(&format!("\t{MOD} v0.1.0\n")), "{merged}");
        assert!(!merged.contains(&format!("{MOD} v0.0.0")), "{merged}");
        assert!(merged.contains(&format!("\t{CORE} v0.0.0\n")), "{merged}");
        assert!(
            merged.contains("golang.org/x/sys v0.37.0 // indirect"),
            "{merged}"
        );
        assert!(
            merged.contains(&format!("\t{MOD} => /abs/golem\n")),
            "{merged}"
        );
        assert_eq!(merged.matches("componentize-go").count(), 1, "{merged}");
        validate(&merged).unwrap();
        assert_eq!(merge_documents(&merged, &update).unwrap(), merged);
    }

    #[test]
    fn merge_go_sum_is_the_sorted_union() {
        let current = "b v1 h1:x=\na v1 h1:y=\n";
        let update = "c v1 h1:z=\na v1 h1:y=\n";
        assert_eq!(
            merge_go_sum(current, update).unwrap(),
            "a v1 h1:y=\nb v1 h1:x=\nc v1 h1:z=\n"
        );
    }

    /// A block closed by `) // comment`, followed by another block, takes the
    /// new requirement inside itself; and a module directive may use any
    /// whitespace.
    #[test]
    fn merge_reads_comments_and_whitespace_as_go_does() {
        let current = format!(
            "module\tapp\n\nrequire (\n\t{MOD} v0.1.0 // sdk\n) // dependencies\n\ntool (\n\tgithub.com/bytecodealliance/componentize-go\n)\n"
        );
        let update = format!("module app\n\nrequire (\n\t{MOD} v0.0.0\n\t{CORE} v0.0.0\n)\n");
        let merged = merge_documents(&current, &update).unwrap();
        assert!(
            merged.contains(&format!("\t{CORE} v0.0.0\n) // dependencies")),
            "{merged}"
        );
        assert!(
            merged.contains("tool (\n\tgithub.com/bytecodealliance/componentize-go\n)"),
            "{merged}"
        );
        validate(&merged).unwrap();
    }

    #[test]
    fn validate_rejects_malformed_entries() {
        assert!(validate("module app\n\ntool (\n\ta b\n)\n").is_err());
        assert!(validate("module app\n\nrequire (\n\ta v1\n").is_err());
        assert!(validate("require a v1\n").is_err());
        assert!(
            validate("module app // the app\n\ngo 1.27.1\n\nrequire a v1 // indirect\n").is_ok()
        );
    }
}

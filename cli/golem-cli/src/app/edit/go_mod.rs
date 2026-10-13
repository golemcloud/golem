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

/// The tokens of one `go.mod` line, as the go command's lexer reads them:
/// separated by whitespace, `(` and `)` as tokens of their own, `//` starting
/// a comment outside a quoted token, and quoted tokens (`"…"` or `` `…` ``)
/// kept whole, quotes included.
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
        } else if b == b'(' || b == b')' {
            out.push((b as char).to_string());
            i += 1;
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
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && bytes[i] != b'('
                && bytes[i] != b')'
                && !comment(i)
            {
                i += 1;
            }
            out.push(line[start..i].to_string());
        }
    }
    out
}

/// The value of a `go.mod` token: a quoted token unquoted as Go does, any
/// other token as written.
fn token_value(token: &str) -> Option<String> {
    if token.starts_with('"') || token.starts_with('`') {
        unquote(token)
    } else {
        Some(token.to_string())
    }
}

/// The value of a Go string literal: a raw `` `…` `` literal as written, an
/// interpreted `"…"` literal with its escapes decoded.
fn unquote(literal: &str) -> Option<String> {
    if let Some(raw) = literal.strip_prefix('`') {
        return raw.strip_suffix('`').map(str::to_string);
    }
    let body = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut bytes = Vec::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let hex = |chars: &mut std::str::Chars, n: usize| -> Option<u32> {
            u32::from_str_radix(&chars.by_ref().take(n).collect::<String>(), 16).ok()
        };
        match chars.next()? {
            'a' => bytes.push(0x07),
            'b' => bytes.push(0x08),
            'f' => bytes.push(0x0c),
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            'v' => bytes.push(0x0b),
            '\\' => bytes.push(b'\\'),
            '"' => bytes.push(b'"'),
            'x' => bytes.push(u8::try_from(hex(&mut chars, 2)?).ok()?),
            'u' => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(
                    char::from_u32(hex(&mut chars, 4)?)?
                        .encode_utf8(&mut buf)
                        .as_bytes(),
                )
            }
            'U' => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(
                    char::from_u32(hex(&mut chars, 8)?)?
                        .encode_utf8(&mut buf)
                        .as_bytes(),
                )
            }
            d @ '0'..='7' => {
                let rest: String = chars.by_ref().take(2).collect();
                bytes.push(u8::from_str_radix(&format!("{d}{rest}"), 8).ok()?);
            }
            _ => return None,
        }
    }
    String::from_utf8(bytes).ok()
}

/// The local directories the `replace` directives of a `go.mod` point to:
/// targets that are a relative (`./`, `../`) or absolute path rather than a
/// module path, as the go command tells them apart.
pub fn local_replacements(content: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_block = false;
    for tokens in content.lines().map(tokens) {
        let entry: &[String] = match tokens.as_slice() {
            [] => continue,
            [kind, open] if kind == "replace" && open == "(" => {
                in_block = true;
                continue;
            }
            [close] if in_block && close == ")" => {
                in_block = false;
                continue;
            }
            _ if in_block => &tokens,
            [kind, rest @ ..] if kind == "replace" => rest,
            _ => continue,
        };
        let Some(arrow) = entry.iter().position(|t| t == "=>") else {
            continue;
        };
        let Some(target) = entry.get(arrow + 1).and_then(|t| token_value(t)) else {
            continue;
        };
        if is_directory_path(&target) {
            targets.push(target);
        }
    }
    targets
}

/// Whether a `replace` target is a directory rather than a module path: the
/// go command's `modfile.IsDirectoryPath`, which checks the Unix and the
/// Windows syntax on every system because a go.mod moves between them.
fn is_directory_path(target: &str) -> bool {
    let bytes = target.as_bytes();
    target == "."
        || target == ".."
        || ["./", ".\\", "../", "..\\", "/", "\\"]
            .iter()
            .any(|prefix| target.starts_with(prefix))
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// Whether a token names `module`, quoted or not.
fn names(token: Option<&String>, module: &str) -> bool {
    token.and_then(|t| token_value(t)).as_deref() == Some(module)
}

/// A `require` entry for `module` — either `require <module> <ver>` (single) or
/// `<module> <ver>` inside a `require (…)` block — has its version set. Lines
/// are read with the go command's tokens, so a quoted module path matches too.
fn set_require_version(lines: &mut [String], module: &str, version: &str) {
    let mut in_require_block = false;
    for line in lines.iter_mut() {
        let toks = tokens(line);
        if !in_require_block && toks == ["require", "("] {
            in_require_block = true;
            continue;
        }
        if in_require_block && toks == [")"] {
            in_require_block = false;
            continue;
        }
        if toks.iter().any(|t| t == "=>") {
            continue; // replace line, not require
        }
        let indent = leading_ws(line).to_string();
        let suffix = trailing_comment(line.trim());
        if toks.first().is_some_and(|t| t == "require") && names(toks.get(1), module) {
            *line = format!("{indent}require {module} {version}{suffix}");
        } else if in_require_block && names(toks.first(), module) {
            *line = format!("{indent}{module} {version}{suffix}");
        }
    }
}

/// Ensure or remove a top-level `replace <module> => <path>` directive.
/// Replace *blocks* are also handled: an entry `<module> => <path>` inside a
/// `replace (…)` block is updated or removed. A quoted module path matches.
fn set_replace_directive(lines: &mut Vec<String>, module: &str, replace_path: Option<&str>) {
    let mut updated = false;
    let mut in_replace_block = false;
    let mut removals = Vec::new();

    for (idx, line) in lines.iter_mut().enumerate() {
        let toks = tokens(line);
        if !in_replace_block && toks == ["replace", "("] {
            in_replace_block = true;
            continue;
        }
        if in_replace_block && toks == [")"] {
            in_replace_block = false;
            continue;
        }
        if !toks.iter().any(|t| t == "=>") {
            continue;
        }
        let is_single = toks.first().is_some_and(|t| t == "replace") && names(toks.get(1), module);
        let is_block_entry = in_replace_block && names(toks.first(), module);

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

    /// Local replacement targets are told from module paths as the go command
    /// does, in the single-line and block forms, quoted or not, with the
    /// parentheses written against the keyword.
    #[test]
    fn local_replacements_are_relative_or_absolute_paths() {
        let content = r#"module app

replace example.com/a => ../a // comment
replace example.com/b v1.0.0 => example.com/fork v1.0.1
replace(
	example.com/c => "../my lib"
	"example.com/d" => ./d
	example.com/e => /abs/e
)
"#;
        assert_eq!(
            local_replacements(content),
            vec!["../a", "../my lib", "./d", "/abs/e"]
        );
    }

    #[test]
    fn tokens_split_parentheses_and_keep_quoted_tokens() {
        assert_eq!(
            tokens("require(a v1)"),
            vec!["require", "(", "a", "v1", ")"]
        );
        assert_eq!(
            tokens(r#"replace "a b" => `c d` // note"#),
            vec!["replace", r#""a b""#, "=>", "`c d`"]
        );
        assert_eq!(tokens("module app/é"), vec!["module", "app/é"]);
    }

    /// A bare parent or current directory is a local replacement too.
    #[test]
    fn a_bare_parent_is_a_local_replacement() {
        assert_eq!(
            local_replacements(
                "module app\n\nreplace localdep => ..\nreplace other => .\nreplace rooted => \\x\nreplace drive => C:\\x\nreplace module => example.com/m v1.0.0\n"
            ),
            vec!["..", ".", "\\x", "C:\\x"]
        );
    }

    /// A quoted module path is the same module: reconciling updates its
    /// replace and require instead of adding a second, conflicting one.
    #[test]
    fn quoted_module_paths_are_reconciled_in_place() {
        let src =
            format!("module app\n\nrequire \"{MOD}\" v0.0.0\n\nreplace \"{MOD}\" => /old/path\n");
        let out = reconcile_sdk_dependency(&src, MOD, "v0.1.0", Some("/new/path"));
        assert_eq!(out.matches("replace").count(), 1, "{out}");
        assert!(
            out.contains(&format!("replace {MOD} => /new/path")),
            "{out}"
        );
        assert!(out.contains(&format!("require {MOD} v0.1.0")), "{out}");
        let removed = reconcile_sdk_dependency(&src, MOD, "v0.1.0", None);
        assert!(!removed.contains("replace"), "{removed}");
    }
}

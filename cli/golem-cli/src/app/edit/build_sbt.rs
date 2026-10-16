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

//! Merges and in-place edits for the build files used by the Scala templates.
//!
//! These edits are intentionally text-based (instead of parsing the sbt DSL)
//! so that arbitrary user customizations in the file are preserved verbatim.
//! [`SbtSource`] removes the comments and splits an sbt file into top-level
//! statements, so an edit always replaces a whole declaration, also when the
//! declaration spans several lines. A declaration of a template-owned setting,
//! plugin or module in a form that the merge does not recognize is an error,
//! because a replacement of only a part of it can break the build.

use anyhow::bail;
use regex::Regex;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::LazyLock;

/// An sbt module id: `"org" % "name" % "version"`, with `%`, `%%` or `%%%` after the org.
static MODULE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""(?P<org>[^"]+)"\s*%{1,3}\s*"(?P<name>[^"]+)"\s*%\s*"(?P<version>[^"]*)""#)
        .unwrap()
});

static THIS_BUILD_SETTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ThisBuild\s*/\s*(?P<key>[A-Za-z0-9_]+)\s*:=").unwrap());

static SBT_PLUGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^addSbtPlugin\s*\(").unwrap());

static COMPONENT_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.in\s*\(\s*file\s*\(\s*"(?P<dir>[^"]*)"\s*\)\s*\)"#).unwrap());

/// The org and the name of a module id, with any version.
fn module_coordinates(org: &str, name: &str) -> Regex {
    Regex::new(&format!(
        r#""{}"\s*%{{1,3}}\s*"{}""#,
        regex::escape(org),
        regex::escape(name)
    ))
    .unwrap()
}

/// Any change of the setting `key` with an sbt setting operator.
fn setting_change(key: &str) -> Regex {
    Regex::new(&format!(
        r"\b{}\s*(?:\+\+=|--=|:=|~=|\+=|-=)",
        regex::escape(key)
    ))
    .unwrap()
}

/// Merges a component sbt file: the current file is kept, and the versions of the
/// modules that the template declares are set to the template's versions.
pub fn merge_component_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    let current_source = SbtSource::parse(current)?;
    let new_source = SbtSource::parse(new)?;

    let new_versions = MODULE_ID
        .captures_iter(&new_source.masked)
        .map(|captures| {
            (
                (captures["org"].to_string(), captures["name"].to_string()),
                captures["version"].to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    for (org, name) in new_versions.keys() {
        let declared = module_coordinates(org, name)
            .find_iter(&current_source.masked)
            .count();
        let recognized = MODULE_ID
            .captures_iter(&current_source.masked)
            .filter(|captures| &captures["org"] == org && &captures["name"] == name)
            .count();
        if declared != recognized {
            bail!(
                "cannot set the version of `\"{org}\" % \"{name}\"`, because the version is not a string literal"
            );
        }
    }

    let mut result = String::with_capacity(current.len());
    let mut pos = 0;
    for captures in MODULE_ID.captures_iter(&current_source.masked) {
        if let Some(new_version) =
            new_versions.get(&(captures["org"].to_string(), captures["name"].to_string()))
        {
            let version = captures.name("version").unwrap();
            result.push_str(&current[pos..version.start()]);
            result.push_str(new_version);
            pos = version.end();
        }
    }
    result.push_str(&current[pos..]);
    Ok(result)
}

/// Merges `build.sbt`: each `ThisBuild / <key> := ...` statement of the template replaces the
/// current statement with the same key, or is appended.
pub fn merge_build_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    merge_keyed_statements(
        current,
        new,
        |statement| {
            THIS_BUILD_SETTING
                .captures(statement)
                .map(|captures| captures["key"].to_string())
        },
        |statement, key| setting_change(key).is_match(statement),
    )
}

/// Merges `project/plugins.sbt`: each `addSbtPlugin(...)` statement of the template replaces the
/// current statement that adds the same plugin, or is appended.
pub fn merge_plugins_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    merge_keyed_statements(
        current,
        new,
        |statement| {
            if !SBT_PLUGIN.is_match(statement) {
                return None;
            }
            let mut module_ids = MODULE_ID.captures_iter(statement);
            match (module_ids.next(), module_ids.next()) {
                (Some(captures), None) => {
                    Some(format!("{}:{}", &captures["org"], &captures["name"]))
                }
                _ => None,
            }
        },
        |statement, key| {
            let (org, name) = key.split_once(':').unwrap();
            SBT_PLUGIN.is_match(statement) || module_coordinates(org, name).is_match(statement)
        },
    )
}

/// Merges a Java properties file such as `project/build.properties`: each property of the
/// template replaces the current property with the same key, or is appended.
pub fn merge_properties(current: &str, new: &str) -> anyhow::Result<String> {
    merge_declarations(current, &properties(current), new, &properties(new))
}

/// Rewrite the argument of the `.in(file("..."))` call of the
/// `lazy val <lazy_val> = project` statement so that it points to `new_dir`.
///
/// The rest of the file is preserved verbatim.
pub fn rewrite_component_dir(
    current: &str,
    lazy_val: &str,
    new_dir: &str,
) -> anyhow::Result<String> {
    let source = SbtSource::parse(current)?;
    let declaration = Regex::new(&format!(r"^lazy\s+val\s+{}\b", regex::escape(lazy_val))).unwrap();

    let spans = source
        .statements
        .iter()
        .filter(|span| declaration.is_match(source.masked(span)))
        .collect::<Vec<_>>();
    let span = match spans.as_slice() {
        [] => bail!(
            "expected to find `lazy val {}` in the sbt file but did not",
            lazy_val
        ),
        [span] => *span,
        _ => bail!(
            "expected exactly one `lazy val {}` declaration in the sbt file, found {}",
            lazy_val,
            spans.len()
        ),
    };

    let Some(dir) = COMPONENT_DIR
        .captures(source.masked(span))
        .and_then(|captures| captures.name("dir"))
    else {
        bail!(
            "expected `.in(file(\"...\"))` inside the `lazy val {}` block but did not find one",
            lazy_val
        );
    };

    Ok(format!(
        "{}{}{}",
        &current[..span.start + dir.start()],
        new_dir,
        &current[span.start + dir.end()..]
    ))
}

/// A keyed declaration in a build file, with the span of its full text.
struct Declaration {
    key: String,
    span: Range<usize>,
}

/// Each declaration of the template replaces the current declaration with the same key, or is
/// appended. The other current declarations and all the text between them stay verbatim.
fn merge_declarations(
    current: &str,
    current_declarations: &[Declaration],
    new: &str,
    new_declarations: &[Declaration],
) -> anyhow::Result<String> {
    let mut replacements = Vec::new();
    let mut appended = Vec::new();

    for new_declaration in new_declarations {
        let text = &new[new_declaration.span.clone()];
        let mut matching = current_declarations
            .iter()
            .filter(|declaration| declaration.key == new_declaration.key);
        match (matching.next(), matching.next()) {
            (Some(declaration), None) => replacements.push((declaration.span.clone(), text)),
            (None, _) => appended.push(text),
            (Some(_), Some(_)) => bail!(
                "cannot merge `{}`, because it is declared more than once",
                new_declaration.key
            ),
        }
    }

    replacements.sort_by_key(|(span, _)| span.start);

    let mut result = String::with_capacity(current.len());
    let mut pos = 0;
    for (span, text) in replacements {
        result.push_str(&current[pos..span.start]);
        result.push_str(text);
        pos = span.end;
    }
    result.push_str(&current[pos..]);

    if !appended.is_empty() {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        for text in appended {
            result.push_str(text);
            result.push('\n');
        }
    }

    Ok(result)
}

/// Merges the keyed statements of the template into the current sbt file. `key` returns the key
/// of a statement in a recognized form. `conflicts` tells if a statement in an other form
/// declares the given template key: the merge cannot replace such a statement safely.
fn merge_keyed_statements(
    current: &str,
    new: &str,
    key: impl Fn(&str) -> Option<String>,
    conflicts: impl Fn(&str, &str) -> bool,
) -> anyhow::Result<String> {
    let current_source = SbtSource::parse(current)?;
    let new_source = SbtSource::parse(new)?;

    let new_declarations = new_source
        .statements
        .iter()
        .filter_map(|span| {
            key(new_source.masked(span)).map(|key| Declaration {
                key,
                span: span.clone(),
            })
        })
        .collect::<Vec<_>>();

    let mut current_declarations = Vec::new();
    for span in &current_source.statements {
        let statement = current_source.masked(span);
        match key(statement) {
            Some(key) => current_declarations.push(Declaration {
                key,
                span: span.clone(),
            }),
            None => {
                if let Some(declaration) = new_declarations
                    .iter()
                    .find(|declaration| conflicts(statement, &declaration.key))
                {
                    bail!(
                        "cannot merge `{}`, because the merge does not recognize the form of this statement: {}",
                        declaration.key,
                        &current[span.clone()]
                    );
                }
            }
        }
    }

    merge_declarations(current, &current_declarations, new, &new_declarations)
}

/// The properties of a Java properties file. The span of a property includes its
/// continuation lines.
fn properties(text: &str) -> Vec<Declaration> {
    let mut result = Vec::<Declaration>::new();
    let mut offset = 0;
    let mut continued = false;

    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let end = start + content.len();

        if continued {
            if let Some(property) = result.last_mut() {
                property.span.end = end;
            }
        } else {
            let trimmed = content.trim_start();
            if trimmed.is_empty() || trimmed.starts_with(['#', '!']) {
                continue;
            }
            result.push(Declaration {
                key: property_key(trimmed),
                span: (end - trimmed.len())..end,
            });
        }

        continued = content.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1;
    }

    result
}

/// The key of a property line: the text before the first unescaped `=`, `:` or whitespace.
fn property_key(line: &str) -> String {
    let mut key = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                key.push(c);
                if let Some(escaped) = chars.next() {
                    key.push(escaped);
                }
            }
            '=' | ':' => break,
            c if c.is_whitespace() => break,
            c => key.push(c),
        }
    }
    key
}

/// An sbt file, with its comments removed and split into top-level statements.
struct SbtSource {
    /// The text of the file, with each comment character, except a newline, replaced by a
    /// space. The byte offsets are the same as in the original text.
    masked: String,
    /// The spans of the top-level statements, without leading and trailing whitespace.
    statements: Vec<Range<usize>>,
}

impl SbtSource {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let bytes = text.as_bytes();
        let mut masked = bytes.to_vec();
        let mut brackets = Vec::<u8>::new();
        let mut line_ends = Vec::<usize>::new();

        let mut i = 0;
        while i < bytes.len() {
            let rest = &bytes[i..];
            if rest.starts_with(b"//") {
                while i < bytes.len() && bytes[i] != b'\n' {
                    masked[i] = b' ';
                    i += 1;
                }
            } else if rest.starts_with(b"/*") {
                let mut nesting = 0;
                loop {
                    let rest = &bytes[i.min(bytes.len())..];
                    if rest.is_empty() {
                        bail!("unterminated block comment");
                    } else if rest.starts_with(b"/*") || rest.starts_with(b"*/") {
                        nesting += if rest[0] == b'/' { 1 } else { -1 };
                        masked[i] = b' ';
                        masked[i + 1] = b' ';
                        i += 2;
                        if nesting == 0 {
                            break;
                        }
                    } else {
                        if bytes[i] != b'\n' {
                            masked[i] = b' ';
                        }
                        i += 1;
                    }
                }
            } else if rest.starts_with(b"\"\"\"") {
                let Some(end) = text[i + 3..].find("\"\"\"") else {
                    bail!("unterminated string literal");
                };
                i += 3 + end + 3;
                while i < bytes.len() && bytes[i] == b'"' {
                    i += 1;
                }
            } else if rest[0] == b'"' {
                i += 1;
                loop {
                    match bytes.get(i) {
                        None | Some(b'\n') => bail!("unterminated string literal"),
                        Some(b'\\') => i += 2,
                        Some(b'"') => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
            } else if rest[0] == b'\'' {
                i += char_literal_len(rest);
            } else {
                match rest[0] {
                    b'(' | b'[' | b'{' => brackets.push(rest[0]),
                    b')' | b']' | b'}' => {
                        let open = match rest[0] {
                            b')' => b'(',
                            b']' => b'[',
                            _ => b'{',
                        };
                        if brackets.pop() != Some(open) {
                            bail!("unbalanced `{}`", rest[0] as char);
                        }
                    }
                    b'\n' if brackets.is_empty() => line_ends.push(i),
                    _ => {}
                }
                i += 1;
            }
        }
        if let Some(open) = brackets.last() {
            bail!("unclosed `{}`", *open as char);
        }

        let masked = String::from_utf8(masked)?;

        let mut statements = Vec::<Range<usize>>::new();
        let mut start = 0;
        for end in line_ends.into_iter().chain([text.len()]) {
            let line = &masked[start..end];
            let line_start = start + (line.len() - line.trim_start().len());
            let line_end = start + line.trim_end().len();
            start = end + 1;
            if line_start >= line_end {
                continue;
            }
            match statements.last_mut() {
                Some(statement)
                    if continues(&masked[statement.clone()], &masked[line_start..line_end]) =>
                {
                    statement.end = line_end;
                }
                _ => statements.push(line_start..line_end),
            }
        }

        Ok(Self { masked, statements })
    }

    fn masked(&self, span: &Range<usize>) -> &str {
        &self.masked[span.clone()]
    }
}

/// The length of the character literal at the start of `rest`, or 1 for a quote that does not
/// start a character literal.
fn char_literal_len(rest: &[u8]) -> usize {
    match rest {
        [b'\'', b'\\', _, tail @ ..] => tail
            .iter()
            .position(|b| *b == b'\'' || *b == b'\n')
            .filter(|pos| tail[*pos] == b'\'')
            .map_or(1, |pos| pos + 4),
        [b'\'', c, b'\'', ..] if *c != b'\n' => 3,
        _ => 1,
    }
}

/// Whether `line` continues the `previous` statement, as in Scala's newline handling: the
/// statement ends with an infix operator or a comma, or the line starts with a member
/// selection, an infix operator or a keyword that cannot start a statement.
fn continues(previous: &str, line: &str) -> bool {
    const ENDING_CHARS: &str = "!#%&*+-/:<=>?@\\^|~,.";
    const STARTING_CHARS: &str = ".,+-*/%&|^:=<>?~";
    const KEYWORDS: &[&str] = &[
        "else", "with", "extends", "match", "yield", "then", "do", "catch", "finally",
    ];

    previous.ends_with(|c| ENDING_CHARS.contains(c))
        || line.starts_with(|c| STARTING_CHARS.contains(c))
        || KEYWORDS.iter().any(|keyword| {
            line.strip_prefix(keyword)
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn flat_sbt() -> String {
        r#"import org.scalajs.linker.interface.ModuleKind

lazy val component_name = project
  .in(file("."))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    name := "component-name",
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    libraryDependencies ++= Seq(
      "cloud.golem" %%% "golem-scala-core"  % "0.1.0",
    ),
  )
"#
        .to_string()
    }

    fn statements(text: &str) -> Vec<String> {
        let source = SbtSource::parse(text).unwrap();
        source
            .statements
            .iter()
            .map(|span| source.masked(span).to_string())
            .collect()
    }

    #[test]
    fn sbt_source_splits_top_level_statements_without_comments() {
        let text = indoc::indoc! {r#"
            // ThisBuild / scalaVersion := "2.13.0"
            import a.b // trailing
            /* block /* nested */ comment
               ThisBuild / scalaVersion := "2.12.0" */
            ThisBuild / scalaVersion :=
              "3.3.0"
            lazy val app = project
              .in(file("app"))
              .settings(
                name := "a//b",
                c := 'x'
              )
            val x = 1 +

              2
        "#};

        let statements = statements(text);

        assert_eq!(statements.len(), 4);
        assert_eq!(statements[0], "import a.b");
        assert_eq!(statements[1], "ThisBuild / scalaVersion :=\n  \"3.3.0\"");
        assert!(statements[2].starts_with("lazy val app = project\n  .in(file(\"app\"))"));
        assert!(statements[2].contains("name := \"a//b\""));
        assert!(statements[2].ends_with(')'));
        assert_eq!(statements[3], "val x = 1 +\n\n  2");
    }

    #[test]
    fn sbt_source_rejects_unbalanced_text() {
        assert!(SbtSource::parse("lazy val a = project.settings(\n").is_err());
        assert!(SbtSource::parse("val a = \"x\n").is_err());
        assert!(SbtSource::parse("/* x\n").is_err());
    }

    #[test]
    fn component_dir_edits_in_place() {
        let edited = rewrite_component_dir(&flat_sbt(), "component_name", "scala-main").unwrap();
        assert!(edited.contains("project\n  .in(file(\"scala-main\"))\n"));
        // user-added settings are preserved
        assert!(edited.contains("scalacOptions += \"-experimental\""));
        assert!(edited.contains("golem-scala-core"));
    }

    #[test]
    fn component_dir_is_idempotent() {
        let once = rewrite_component_dir(&flat_sbt(), "component_name", "scala-main").unwrap();
        let twice = rewrite_component_dir(&once, "component_name", "scala-main").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn component_dir_preserves_user_added_dependencies() {
        let with_user_edit = flat_sbt().replace(
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\",\n",
        );
        let edited =
            rewrite_component_dir(&with_user_edit, "component_name", "scala-main").unwrap();
        assert!(edited.contains("\"dev.zio\" %%% \"zio\""));
        assert!(edited.contains(".in(file(\"scala-main\"))"));
    }

    #[test]
    fn component_dir_ignores_commented_out_declarations() {
        let current = format!(
            "// lazy val component_name = project.in(file(\"old\"))\n/* .in(file(\"older\")) */\n{}",
            flat_sbt()
        );
        let edited = rewrite_component_dir(&current, "component_name", "scala-main").unwrap();
        assert_eq!(
            edited,
            current.replace(".in(file(\".\"))", ".in(file(\"scala-main\"))")
        );
    }

    #[test]
    fn component_dir_errors_when_lazy_val_missing() {
        let err = rewrite_component_dir("// nothing here\n", "component_name", "x").unwrap_err();
        assert!(err.to_string().contains("lazy val component_name"));
    }

    #[test]
    fn component_dir_errors_when_in_file_missing() {
        let src = "lazy val component_name = project\n  .settings()\n";
        let err = rewrite_component_dir(src, "component_name", "x").unwrap_err();
        assert!(err.to_string().contains(".in(file"));
    }

    #[test]
    fn component_sbt_merge_keeps_user_edits_and_sets_template_versions() {
        let current = flat_sbt().replace(
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\",\n",
        );
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        let merged = merge_component_sbt(&current, &new).unwrap();

        assert!(merged.contains("\"cloud.golem\" %%% \"golem-scala-core\"  % \"0.2.0\""));
        assert!(merged.contains("\"dev.zio\" %%% \"zio\" % \"2.1.0\""));
        assert!(merged.contains(".in(file(\".\"))"));
        assert_eq!(merge_component_sbt(&merged, &new).unwrap(), merged);
        assert_eq!(
            merge_component_sbt(&flat_sbt(), &flat_sbt()).unwrap(),
            flat_sbt()
        );
    }

    #[test]
    fn component_sbt_merge_sets_multiline_versions_and_skips_comments() {
        let current = flat_sbt().replace(
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n",
            "      // \"cloud.golem\" %%% \"golem-scala-core\" % \"0.0.1\",\n      \"cloud.golem\" %%% \"golem-scala-core\" %\n        \"0.1.0\",\n",
        );
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        let merged = merge_component_sbt(&current, &new).unwrap();

        assert_eq!(
            merged,
            current.replace("        \"0.1.0\"", "        \"0.2.0\"")
        );
    }

    #[test]
    fn component_sbt_merge_rejects_versions_that_are_not_literals() {
        let current = flat_sbt().replace("% \"0.1.0\"", "% golemVersion");
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        let err = merge_component_sbt(&current, &new).unwrap_err();

        assert!(err.to_string().contains("golem-scala-core"));
    }

    #[test]
    fn build_sbt_merge_replaces_template_settings_and_keeps_user_settings() {
        let current = "ThisBuild / scalaVersion := \"3.3.0\"\nThisBuild / organization := \"me\"\n";
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        let merged = merge_build_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            "ThisBuild / scalaVersion := \"3.7.1\"\nThisBuild / organization := \"me\"\n"
        );
        assert_eq!(merge_build_sbt(&merged, new).unwrap(), merged);
        assert_eq!(
            merge_build_sbt("ThisBuild / organization := \"me\"\n", new).unwrap(),
            "ThisBuild / organization := \"me\"\nThisBuild / scalaVersion := \"3.7.1\"\n"
        );
    }

    #[test]
    fn build_sbt_merge_replaces_multiline_settings_and_skips_comments() {
        let current = indoc::indoc! {r#"
            // ThisBuild / scalaVersion := "2.13.0"
            ThisBuild / scalaVersion :=
              "3.3.0"
            ThisBuild / organization := "me"
        "#};
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        let merged = merge_build_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            indoc::indoc! {r#"
                // ThisBuild / scalaVersion := "2.13.0"
                ThisBuild / scalaVersion := "3.7.1"
                ThisBuild / organization := "me"
            "#}
        );
    }

    #[test]
    fn build_sbt_merge_rejects_unrecognized_template_settings() {
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        for current in [
            "inThisBuild(Seq(scalaVersion := \"3.3.0\"))\n",
            "scalaVersion := \"3.3.0\"\n",
            "ThisBuild / scalaVersion := \"3.3.0\"\nThisBuild / scalaVersion := \"3.3.1\"\n",
        ] {
            let err = merge_build_sbt(current, new).unwrap_err();
            assert!(err.to_string().contains("scalaVersion"), "{current}: {err}");
        }

        assert!(
            merge_build_sbt(
                "ThisBuild / crossScalaVersions := Seq(scalaVersion.value)\n",
                new
            )
            .is_ok()
        );
    }

    #[test]
    fn plugins_sbt_merge_sets_template_plugin_versions_and_keeps_user_plugins() {
        let current = indoc::indoc! {r#"
            addSbtPlugin("cloud.golem" % "golem-scala-sbt" % "0.1.0")
            addSbtPlugin("org.scalameta" % "sbt-scalafmt" % "2.5.2")
        "#};
        let new = indoc::indoc! {r#"
            addSbtPlugin("cloud.golem" % "golem-scala-sbt" % "0.2.0")
            addSbtPlugin("org.scala-js" % "sbt-scalajs" % "1.19.0")
        "#};

        let merged = merge_plugins_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            indoc::indoc! {r#"
                addSbtPlugin("cloud.golem" % "golem-scala-sbt" % "0.2.0")
                addSbtPlugin("org.scalameta" % "sbt-scalafmt" % "2.5.2")
                addSbtPlugin("org.scala-js" % "sbt-scalajs" % "1.19.0")
            "#}
        );
        assert_eq!(merge_plugins_sbt(&merged, new).unwrap(), merged);
    }

    #[test]
    fn plugins_sbt_merge_replaces_multiline_plugins() {
        let current = indoc::indoc! {r#"
            addSbtPlugin(
              "cloud.golem" % "golem-scala-sbt" % "0.1.0"
            )
            addSbtPlugin("org.scalameta" % "sbt-scalafmt" % "2.5.2")
        "#};
        let new = "addSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % \"0.2.0\")\n";

        let merged = merge_plugins_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            indoc::indoc! {r#"
                addSbtPlugin("cloud.golem" % "golem-scala-sbt" % "0.2.0")
                addSbtPlugin("org.scalameta" % "sbt-scalafmt" % "2.5.2")
            "#}
        );
    }

    #[test]
    fn plugins_sbt_merge_rejects_unrecognized_plugins() {
        let new = "addSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % \"0.2.0\")\n";

        for current in [
            "val golemVersion = \"0.1.0\"\naddSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % golemVersion)\n",
            "addSbtPlugin(myPlugin)\n",
            "val golemPlugin = \"cloud.golem\" % \"golem-scala-sbt\" % \"0.1.0\"\naddSbtPlugin(golemPlugin)\n",
        ] {
            let err = merge_plugins_sbt(current, new).unwrap_err();
            assert!(
                err.to_string().contains("cloud.golem:golem-scala-sbt"),
                "{current}: {err}"
            );
        }
    }

    #[test]
    fn properties_merge_sets_template_properties_and_keeps_user_properties() {
        let current = "# sbt\nsbt.version=1.10.0\nsbt.color=always\n";
        let new = "sbt.version=1.11.2\n";

        let merged = merge_properties(current, new).unwrap();

        assert_eq!(merged, "# sbt\nsbt.version=1.11.2\nsbt.color=always\n");
        assert_eq!(merge_properties(&merged, new).unwrap(), merged);
    }

    #[test]
    fn properties_merge_replaces_continued_properties() {
        let current = "sbt.version = \\\n  1.10.0\nsbt.color:always\n# sbt.version=0.13\n";
        let new = "sbt.version=1.11.2\n";

        let merged = merge_properties(current, new).unwrap();

        assert_eq!(
            merged,
            "sbt.version=1.11.2\nsbt.color:always\n# sbt.version=0.13\n"
        );
        assert!(merge_properties("sbt.version=1.10.0\nsbt.version 1.10.1\n", new).is_err());
    }
}

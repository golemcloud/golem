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
//! The sbt edits find the declarations in the tree-sitter-scala syntax tree and replace their
//! text spans, so that all other user customizations in the file stay verbatim. A merge
//! replaces only a complete top-level statement in a recognized form with plain string
//! literals. Any other use of a template-owned setting, plugin or module is an error, because
//! the merge cannot replace it safely. A file that does not parse is also an error.

use anyhow::{anyhow, bail};
use std::collections::BTreeMap;
use std::ops::Range;
use tree_sitter::{Node, Parser, Tree};

/// Merges a component sbt file: the current file is kept, and the versions of the
/// modules that the template declares are set to the template's versions.
pub fn merge_component_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    let current_tree = parse_sbt(current)?;
    let new_tree = parse_sbt(new)?;

    let new_versions = descendants(new_tree.root_node())
        .into_iter()
        .filter_map(|node| {
            let ((org, name), version) = module_id(node, new)?;
            Some((
                (org.to_string(), name.to_string()),
                plain_string(version, new)?.to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    let current_nodes = descendants(current_tree.root_node());
    let mut replacements = Vec::new();
    for ((org, name), new_version) in &new_versions {
        let mut recognized = 0;
        for node in &current_nodes {
            if module_coordinates(*node, current) != Some((org.as_str(), name.as_str())) {
                continue;
            }
            let version = node
                .parent()
                .filter(|parent| parent.child_by_field_name("left") == Some(*node))
                .and_then(|parent| module_id(parent, current))
                .and_then(|(_, version)| plain_string(version, current).map(|_| version));
            let Some(version) = version else {
                bail!(
                    "cannot set the version of `\"{org}\" % \"{name}\"`, because the version is not a plain string literal"
                );
            };
            replacements.push((string_contents(version), new_version.as_str()));
            recognized += 1;
        }

        let mentions = current_nodes
            .iter()
            .filter(|node| mentions_string(**node, current, name))
            .count();
        if mentions != recognized {
            bail!(
                "cannot set the version of `\"{org}\" % \"{name}\"`, because the merge does not recognize the form of its declaration"
            );
        }
    }

    let merged = splice(current, replacements);
    parse_sbt(&merged)?;
    Ok(merged)
}

/// Whether `node` is a string literal of any kind, plain, triple-quoted or interpolated,
/// with the contents `contents`.
fn mentions_string(node: Node<'_>, source: &str, contents: &str) -> bool {
    matches!(node.kind(), "string" | "interpolated_string")
        && source[node.byte_range()].trim_matches('"') == contents
}

/// Merges `build.sbt`: each `ThisBuild / <key> := "<value>"` statement of the template replaces
/// the current statement with the same key, or is appended. Any other use of a template key,
/// except a read of its value, is an error.
pub fn merge_build_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    merge_keyed_statements(
        current,
        new,
        |node, source| this_build_setting(node, source).map(str::to_string),
        |node, source, key| {
            descendants(node)
                .into_iter()
                .any(|node| identifier(node, source) == Some(key) && !is_value_read(node, source))
        },
    )
}

/// Merges `project/plugins.sbt`: each `addSbtPlugin("org" % "name" % "version")` statement of
/// the template replaces the current statement that adds the same plugin, or is appended. Any
/// other `addSbtPlugin` statement, and any other use of a template plugin, is an error.
pub fn merge_plugins_sbt(current: &str, new: &str) -> anyhow::Result<String> {
    merge_keyed_statements(current, new, sbt_plugin, |node, source, key| {
        descendants(node).into_iter().any(|node| {
            identifier(node, source) == Some("addSbtPlugin")
                || module_coordinates(node, source)
                    .is_some_and(|(org, name)| format!("{org}:{name}") == key)
        })
    })
}

/// Merges a Java properties file such as `project/build.properties`: each property of the
/// template replaces the current property with the same key, or is appended.
pub fn merge_properties(current: &str, new: &str) -> anyhow::Result<String> {
    let new_properties = properties(new).0;
    let (current_properties, continued) = properties(current);

    // An appended property must not become a continuation line of the last current property:
    // an empty line ends the continuation without changing the value of that property.
    let appends = new_properties.iter().any(|new_property| {
        !current_properties
            .iter()
            .any(|property| property.key == new_property.key)
    });
    let current = if continued && appends {
        format!(
            "{current}{}\n",
            if current.ends_with('\n') { "" } else { "\n" }
        )
    } else {
        current.to_string()
    };

    let merged = merge_declarations(&current, &properties(&current).0, new, &new_properties)?;
    ensure_merged(&merged, &properties(&merged).0, new, &new_properties)?;
    Ok(merged)
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
    let tree = parse_sbt(current)?;

    let declarations = statements(&tree)
        .into_iter()
        .filter(|node| {
            node.kind() == "val_definition"
                && node
                    .child_by_field_name("pattern")
                    .and_then(|pattern| identifier(pattern, current))
                    == Some(lazy_val)
        })
        .collect::<Vec<_>>();
    let declaration = match declarations.as_slice() {
        [] => bail!(
            "expected to find `lazy val {}` in the sbt file but did not",
            lazy_val
        ),
        [declaration] => *declaration,
        _ => bail!(
            "expected exactly one `lazy val {}` declaration in the sbt file, found {}",
            lazy_val,
            declarations.len()
        ),
    };

    let in_calls = descendants(declaration)
        .into_iter()
        .filter(|node| is_in_call(*node, current))
        .collect::<Vec<_>>();
    let dir = match in_calls.as_slice() {
        [in_call] => component_dir(*in_call, current),
        [] => None,
        _ => bail!(
            "expected exactly one `.in(file(\"...\"))` inside the `lazy val {}` block, found {} `.in(...)` calls",
            lazy_val,
            in_calls.len()
        ),
    };
    let Some(dir) = dir else {
        bail!(
            "expected `.in(file(\"...\"))` inside the `lazy val {}` block but did not find one",
            lazy_val
        );
    };

    Ok(splice(current, vec![(string_contents(dir), new_dir)]))
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

    let mut result = splice(current, replacements);

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
/// of a top-level statement in a recognized form. `conflicts` tells if a statement in an other
/// form uses the given template key: the merge cannot replace such a statement safely.
fn merge_keyed_statements(
    current: &str,
    new: &str,
    key: impl Fn(Node<'_>, &str) -> Option<String>,
    conflicts: impl Fn(Node<'_>, &str, &str) -> bool,
) -> anyhow::Result<String> {
    let new_tree = parse_sbt(new)?;
    let new_declarations = statements(&new_tree)
        .into_iter()
        .filter_map(|node| {
            key(node, new).map(|key| Declaration {
                key,
                span: node.byte_range(),
            })
        })
        .collect::<Vec<_>>();

    let current_declarations = keyed_statements(current, &new_declarations, &key, &conflicts)?;
    let merged = merge_declarations(current, &current_declarations, new, &new_declarations)?;

    let merged_declarations = keyed_statements(&merged, &new_declarations, &key, &conflicts)?;
    ensure_merged(&merged, &merged_declarations, new, &new_declarations)?;

    Ok(merged)
}

/// The statements of `source` in a recognized form, with their keys. A statement in an other
/// form that uses a key of `new_declarations` is an error.
fn keyed_statements(
    source: &str,
    new_declarations: &[Declaration],
    key: &impl Fn(Node<'_>, &str) -> Option<String>,
    conflicts: &impl Fn(Node<'_>, &str, &str) -> bool,
) -> anyhow::Result<Vec<Declaration>> {
    let tree = parse_sbt(source)?;
    let mut declarations = Vec::new();
    for node in statements(&tree) {
        match key(node, source) {
            Some(key) => declarations.push(Declaration {
                key,
                span: node.byte_range(),
            }),
            None => {
                if let Some(declaration) = new_declarations
                    .iter()
                    .find(|declaration| conflicts(node, source, &declaration.key))
                {
                    bail!(
                        "cannot merge `{}`, because the merge does not recognize the form of this statement: {}",
                        declaration.key,
                        &source[node.byte_range()]
                    );
                }
            }
        }
    }
    Ok(declarations)
}

/// Checks the merge result: the merged file declares each key of the template exactly once,
/// with the text of the template.
fn ensure_merged(
    merged: &str,
    merged_declarations: &[Declaration],
    new: &str,
    new_declarations: &[Declaration],
) -> anyhow::Result<()> {
    for new_declaration in new_declarations {
        let mut matching = merged_declarations
            .iter()
            .filter(|declaration| declaration.key == new_declaration.key);
        match (matching.next(), matching.next()) {
            (Some(declaration), None)
                if merged[declaration.span.clone()] == new[new_declaration.span.clone()] => {}
            _ => bail!(
                "cannot merge `{}`, because the merged file does not declare it exactly once with the template value",
                new_declaration.key
            ),
        }
    }
    Ok(())
}

/// Replaces the given non-overlapping spans of `text`.
fn splice(text: &str, mut replacements: Vec<(Range<usize>, &str)>) -> String {
    replacements.sort_by_key(|(span, _)| span.start);

    let mut result = String::with_capacity(text.len());
    let mut pos = 0;
    for (span, replacement) in replacements {
        result.push_str(&text[pos..span.start]);
        result.push_str(replacement);
        pos = span.end;
    }
    result.push_str(&text[pos..]);
    result
}

/// The properties of a Java properties file, and whether the last line of the file continues
/// a property. The span of a property includes its continuation lines.
fn properties(text: &str) -> (Vec<Declaration>, bool) {
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

    (result, continued)
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

fn parse_sbt(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_scala::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-scala"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse sbt"))?;
    if tree.root_node().has_error() {
        bail!("Invalid sbt, or a form that the Scala grammar does not support");
    }
    Ok(tree)
}

/// The top-level statements of an sbt file, without the comments.
fn statements(tree: &Tree) -> Vec<Node<'_>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|node| !node.is_extra())
        .collect()
}

/// `node` and all its named descendants, comments included.
fn descendants(node: Node<'_>) -> Vec<Node<'_>> {
    let mut result = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        result.push(node);
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    result
}

/// The name of an identifier, without the backticks of a quoted identifier.
fn identifier<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    if node.kind() != "identifier" {
        return None;
    }
    let text = &source[node.byte_range()];
    Some(
        text.strip_prefix('`')
            .and_then(|text| text.strip_suffix('`'))
            .unwrap_or(text),
    )
}

/// The contents of a plain string literal: `"..."`, not triple-quoted, not interpolated and
/// without escapes.
fn plain_string<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    if node.kind() != "string" {
        return None;
    }
    let contents = source[node.byte_range()]
        .strip_prefix('"')?
        .strip_suffix('"')?;
    (!contents.contains(['"', '\\'])).then_some(contents)
}

/// The span of the contents of a plain string literal, without the quotes.
fn string_contents(node: Node<'_>) -> Range<usize> {
    node.start_byte() + 1..node.end_byte() - 1
}

/// The left operand, the operator and the right operand of an infix expression.
fn infix<'t, 'a>(node: Node<'t>, source: &'a str) -> Option<(Node<'t>, &'a str, Node<'t>)> {
    if node.kind() != "infix_expression" {
        return None;
    }
    Some((
        node.child_by_field_name("left")?,
        &source[node.child_by_field_name("operator")?.byte_range()],
        node.child_by_field_name("right")?,
    ))
}

/// The only argument of a call of the function `function`.
fn single_argument<'t>(node: Node<'t>, source: &str, function: &str) -> Option<Node<'t>> {
    if node.kind() != "call_expression"
        || identifier(node.child_by_field_name("function")?, source)? != function
    {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let arguments = arguments
        .named_children(&mut cursor)
        .filter(|node| !node.is_extra())
        .collect::<Vec<_>>();
    match arguments.as_slice() {
        [argument] => Some(*argument),
        _ => None,
    }
}

/// `ThisBuild / <key> := "<value>"`: the key.
fn this_build_setting<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    let (scoped_key, ":=", value) = infix(node, source)? else {
        return None;
    };
    plain_string(value, source)?;
    let (scope, "/", key) = infix(scoped_key, source)? else {
        return None;
    };
    if identifier(scope, source)? != "ThisBuild" {
        return None;
    }
    identifier(key, source)
}

/// Whether the setting identifier `node` is only read, as in `key.value` or
/// `(ThisBuild / key).value`.
fn is_value_read(node: Node<'_>, source: &str) -> bool {
    let mut expression = node;
    if let Some(parent) = expression.parent()
        && infix(parent, source)
            .is_some_and(|(_, operator, right)| operator == "/" && right == expression)
    {
        expression = parent;
    }
    if let Some(parent) = expression.parent()
        && parent.kind() == "parenthesized_expression"
    {
        expression = parent;
    }
    expression.parent().is_some_and(|parent| {
        parent.kind() == "field_expression"
            && parent.child_by_field_name("value") == Some(expression)
            && parent
                .child_by_field_name("field")
                .and_then(|field| identifier(field, source))
                == Some("value")
    })
}

/// `"org" % "name"`, with `%`, `%%` or `%%%` and plain string literals: the org and the name.
fn module_coordinates<'a>(node: Node<'_>, source: &'a str) -> Option<(&'a str, &'a str)> {
    let (org, operator, name) = infix(node, source)?;
    if !matches!(operator, "%" | "%%" | "%%%") {
        return None;
    }
    Some((plain_string(org, source)?, plain_string(name, source)?))
}

/// `"org" % "name" % <version>`: the org, the name and the version expression.
fn module_id<'t, 'a>(node: Node<'t>, source: &'a str) -> Option<((&'a str, &'a str), Node<'t>)> {
    let (coordinates, "%", version) = infix(node, source)? else {
        return None;
    };
    Some((module_coordinates(coordinates, source)?, version))
}

/// `addSbtPlugin("org" % "name" % "version")` with plain string literals: `org:name`.
fn sbt_plugin(node: Node<'_>, source: &str) -> Option<String> {
    let argument = single_argument(node, source, "addSbtPlugin")?;
    let ((org, name), version) = module_id(argument, source)?;
    plain_string(version, source)?;
    Some(format!("{org}:{name}"))
}

/// Whether `node` is an `<expression>.in(...)` call.
fn is_in_call(node: Node<'_>, source: &str) -> bool {
    node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .filter(|function| function.kind() == "field_expression")
            .and_then(|function| function.child_by_field_name("field"))
            .and_then(|field| identifier(field, source))
            == Some("in")
}

/// `<project>.in(file("<dir>"))`: the string literal of the directory.
fn component_dir<'t>(node: Node<'t>, source: &str) -> Option<Node<'t>> {
    if !is_in_call(node, source) {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let arguments = arguments
        .named_children(&mut cursor)
        .filter(|node| !node.is_extra())
        .collect::<Vec<_>>();
    let [file] = arguments.as_slice() else {
        return None;
    };
    let dir = single_argument(*file, source, "file")?;
    plain_string(dir, source)?;
    Some(dir)
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
      "cloud.golem" %%% "golem-scala-core"  % "0.1.0"
    )
  )
"#
        .to_string()
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
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\"\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\"\n",
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
    fn component_dir_errors_when_more_than_one_in_call() {
        for src in [
            "lazy val component_name = project.in(file(\"a\")).in(file(\"b\"))\n",
            "lazy val component_name = (if (c) other.in(file(\"a\")) else project).in(file(\".\"))\n",
            "lazy val component_name = project.in(file(dir)).in(file(\".\"))\n",
        ] {
            let err = rewrite_component_dir(src, "component_name", "x").unwrap_err();
            assert!(err.to_string().contains("exactly one"), "{src}: {err}");
        }
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
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\"\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\"\n",
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
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\"\n",
            "      // \"cloud.golem\" %%% \"golem-scala-core\" % \"0.0.1\",\n      \"cloud.golem\" %%% \"golem-scala-core\" %\n        \"0.1.0\"\n",
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
    fn component_sbt_merge_rejects_triple_quoted_versions() {
        let current =
            "libraryDependencies += \"cloud.golem\" %% \"golem-scala-core\" % \"\"\"0.1.0\"\"\"\n";
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        let err = merge_component_sbt(current, &new).unwrap_err();

        assert!(err.to_string().contains("golem-scala-core"), "{err}");
    }

    #[test]
    fn component_sbt_merge_rejects_unrecognized_module_declarations() {
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        for current in [
            "val golemOrg = \"cloud.golem\"\nlibraryDependencies += golemOrg %%% \"golem-scala-core\" % \"0.1.0\"\n",
            "libraryDependencies += \"cloud.golem\" %%% s\"golem-scala-core\" % \"0.1.0\"\n",
            "libraryDependencies += \"cloud.golem\".%%%(\"golem-scala-core\").%(\"0.1.0\")\n",
            "libraryDependencies += (\"cloud.golem\" %%% \"golem-scala-core\") % \"0.1.0\"\n",
        ] {
            let err = merge_component_sbt(current, &new).unwrap_err();
            assert!(
                err.to_string().contains("golem-scala-core"),
                "{current}: {err}"
            );
        }
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
    fn build_sbt_merge_keeps_settings_after_a_semicolon() {
        let current =
            "ThisBuild / scalaVersion := \"3.3.0\"; ThisBuild / organization := \"custom\"\n";
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        let merged = merge_build_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            "ThisBuild / scalaVersion := \"3.7.1\"; ThisBuild / organization := \"custom\"\n"
        );
        assert_eq!(merge_build_sbt(&merged, new).unwrap(), merged);
    }

    #[test]
    fn build_sbt_merge_replaces_multiline_settings_and_skips_comments() {
        let current = indoc::indoc! {r#"
            // ThisBuild / scalaVersion := "2.13.0"
            /* ThisBuild / scalaVersion := "2.12.0" */
            ThisBuild / scalaVersion :=
              "3.3.0" // the version
            ThisBuild / organization := "me"
        "#};
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        let merged = merge_build_sbt(current, new).unwrap();

        assert_eq!(
            merged,
            indoc::indoc! {r#"
                // ThisBuild / scalaVersion := "2.13.0"
                /* ThisBuild / scalaVersion := "2.12.0" */
                ThisBuild / scalaVersion := "3.7.1" // the version
                ThisBuild / organization := "me"
            "#}
        );
    }

    #[test]
    fn build_sbt_merge_rejects_files_that_do_not_parse() {
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        for current in [
            "lazy val a = project.settings(\n",
            "ThisBuild / scalaVersion := \"3.3.0\" /* comment\n*/ ThisBuild / organization := \"example\"\n",
        ] {
            let err = merge_build_sbt(current, new).unwrap_err();
            assert!(err.to_string().contains("Invalid sbt"), "{current}: {err}");
        }
    }

    #[test]
    fn build_sbt_merge_rejects_unrecognized_template_settings() {
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        for current in [
            "inThisBuild(Seq(scalaVersion := \"3.3.0\"))\n",
            "scalaVersion := \"3.3.0\"\n",
            "ThisBuild / scalaVersion := \"3.3.0\"\nThisBuild / scalaVersion := \"3.3.1\"\n",
            "ThisBuild / scalaVersion := \"3.3.0\"\nscalaVersion in ThisBuild := \"3.6.4\"\n",
            "ThisBuild / scalaVersion := sys.props.get(\"scala.version\") getOrElse\n  \"3.3.0\"\n",
            "ThisBuild / scalaVersion := \"3\" + \".3.0\"\n",
            "ThisBuild / scalaVersion := \"\"\"3.3.0\"\"\"\n",
            "ThisBuild / scalaVersion := s\"3.3.${patch}\"\n",
            "ThisBuild / scalaVersion := \"3.3.0\"\nThisBuild / `scalaVersion` := \"2.13.12\"\n",
            "lazy val a = project.settings(`scalaVersion` := \"2.13.12\")\n",
        ] {
            let err = merge_build_sbt(current, new).unwrap_err();
            assert!(err.to_string().contains("scalaVersion"), "{current}: {err}");
        }

        assert!(
            merge_build_sbt(
                "ThisBuild / crossScalaVersions := Seq(scalaVersion.value, (ThisBuild / scalaVersion).value)\n",
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
            "addSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % \"\"\"0.1.0\"\"\")\n",
            "addSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % \"0.1.0\" exclude(\"a\", \"b\"))\n",
            "addSbtPlugin(\"cloud.golem\" % \"golem-scala-sbt\" % \"0.1.0\") getOrElse\n  x\n",
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

    #[test]
    fn properties_merge_ends_a_continued_last_property_before_appending() {
        let new = "sbt.version=1.11.2\n";

        for current in ["x=1\\\n", "x=1\\"] {
            let merged = merge_properties(current, new).unwrap();

            assert_eq!(merged, "x=1\\\n\nsbt.version=1.11.2\n", "{current:?}");
            assert_eq!(merge_properties(&merged, new).unwrap(), merged);
        }
    }

    #[test]
    fn build_sbt_merge_replaces_backtick_quoted_settings() {
        let merged = merge_build_sbt(
            "ThisBuild / `scalaVersion` := \"3.3.0\"\n",
            "ThisBuild / scalaVersion := \"3.7.1\"\n",
        )
        .unwrap();

        assert_eq!(merged, "ThisBuild / scalaVersion := \"3.7.1\"\n");
    }
}

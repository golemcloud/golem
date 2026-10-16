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

//! In-place edits for sbt build files used by the Scala templates.
//!
//! These edits are intentionally text-based (instead of parsing the sbt DSL)
//! so that arbitrary user customizations in the file are preserved verbatim.

use anyhow::anyhow;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// An sbt module id: `"org" % "name" % "version"`, with `%`, `%%` or `%%%` after the org.
static MODULE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""(?P<org>[^"]+)"\s*%{1,3}\s*"(?P<name>[^"]+)"\s*%\s*"(?P<version>[^"]*)""#)
        .unwrap()
});

static THIS_BUILD_SETTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*ThisBuild\s*/\s*(?P<key>[A-Za-z0-9_]+)\s*:=").unwrap());

static SBT_PLUGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*addSbtPlugin\s*\(").unwrap());

static PROPERTY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?P<key>[^#!\s=:][^\s=:]*)\s*[=:]").unwrap());

/// Merges a component sbt file: the current file is kept, and the versions of the
/// modules that the template declares are set to the template's versions.
pub fn merge_component_sbt(current: &str, new: &str) -> String {
    let new_versions = MODULE_ID
        .captures_iter(new)
        .map(|captures| {
            (
                (captures["org"].to_string(), captures["name"].to_string()),
                captures["version"].to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    MODULE_ID
        .replace_all(current, |captures: &regex::Captures| {
            let module_id = captures.get(0).unwrap();
            match new_versions.get(&(captures["org"].to_string(), captures["name"].to_string())) {
                Some(new_version) => {
                    let version = captures.name("version").unwrap();
                    format!(
                        "{}{}{}",
                        &current[module_id.start()..version.start()],
                        new_version,
                        &current[version.end()..module_id.end()]
                    )
                }
                None => module_id.as_str().to_string(),
            }
        })
        .into_owned()
}

/// Merges `build.sbt`: each `ThisBuild / <key> := ...` line of the template replaces the
/// current line with the same key, or is appended.
pub fn merge_build_sbt(current: &str, new: &str) -> String {
    merge_keyed_lines(current, new, |line| {
        THIS_BUILD_SETTING
            .captures(line)
            .map(|captures| captures["key"].to_string())
    })
}

/// Merges `project/plugins.sbt`: each `addSbtPlugin(...)` line of the template replaces the
/// current line that adds the same plugin, or is appended.
pub fn merge_plugins_sbt(current: &str, new: &str) -> String {
    merge_keyed_lines(current, new, |line| {
        if !SBT_PLUGIN.is_match(line) {
            return None;
        }
        MODULE_ID
            .captures(line)
            .map(|captures| format!("{}:{}", &captures["org"], &captures["name"]))
    })
}

/// Merges a Java properties file such as `project/build.properties`: each property of the
/// template replaces the current line with the same key, or is appended.
pub fn merge_properties(current: &str, new: &str) -> String {
    merge_keyed_lines(current, new, |line| {
        PROPERTY
            .captures(line)
            .map(|captures| captures["key"].to_string())
    })
}

fn merge_keyed_lines(current: &str, new: &str, key: impl Fn(&str) -> Option<String>) -> String {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();

    for new_line in new.lines() {
        let Some(new_key) = key(new_line) else {
            continue;
        };
        match lines
            .iter()
            .position(|line| key(line).as_deref() == Some(new_key.as_str()))
        {
            Some(idx) => lines[idx] = new_line.to_string(),
            None => lines.push(new_line.to_string()),
        }
    }

    let mut result = lines.join("\n");
    if !result.is_empty() && (current.ends_with('\n') || current.is_empty()) {
        result.push('\n');
    }
    result
}

/// Rewrite the argument of the first `.in(file("..."))` call inside the
/// `lazy val <lazy_val> = project` block so that it points to `new_dir`.
///
/// The rest of the file is preserved verbatim.
pub fn rewrite_component_dir(
    current: &str,
    lazy_val: &str,
    new_dir: &str,
) -> anyhow::Result<String> {
    // Locate the `lazy val <lazy_val>` declaration.
    let needle = format!("lazy val {}", lazy_val);
    let lazy_val_offsets: Vec<usize> = current.match_indices(&needle).map(|(i, _)| i).collect();
    if lazy_val_offsets.is_empty() {
        return Err(anyhow!(
            "expected to find `lazy val {}` in the sbt file but did not",
            lazy_val
        ));
    }
    if lazy_val_offsets.len() > 1 {
        return Err(anyhow!(
            "expected exactly one `lazy val {}` declaration in the sbt file, found {}",
            lazy_val,
            lazy_val_offsets.len()
        ));
    }

    let lazy_val_start = lazy_val_offsets[0];

    // From the lazy val onwards, find the first `.in(file("..."))` call and replace
    // its quoted argument.
    let after = &current[lazy_val_start..];
    let prefix = ".in(file(\"";
    let Some(in_call_offset_in_after) = after.find(prefix) else {
        return Err(anyhow!(
            "expected `.in(file(\"...\"))` inside the `lazy val {}` block but did not find one",
            lazy_val
        ));
    };
    let arg_start_in_current = lazy_val_start + in_call_offset_in_after + prefix.len();

    let after_arg = &current[arg_start_in_current..];
    let Some(arg_end_in_after) = after_arg.find('"') else {
        return Err(anyhow!(
            "unterminated string literal in `.in(file(\"...\"))` for `lazy val {}`",
            lazy_val
        ));
    };
    let arg_end_in_current = arg_start_in_current + arg_end_in_after;

    let mut result = String::with_capacity(current.len() + new_dir.len());
    result.push_str(&current[..arg_start_in_current]);
    result.push_str(new_dir);
    result.push_str(&current[arg_end_in_current..]);
    Ok(result)
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
    fn component_dir_errors_when_lazy_val_missing() {
        let err = rewrite_component_dir("// nothing here\n", "component_name", "x").unwrap_err();
        assert!(err.to_string().contains("lazy val component_name"));
    }

    #[test]
    fn component_sbt_merge_keeps_user_edits_and_sets_template_versions() {
        let current = flat_sbt().replace(
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\",\n",
        );
        let new = flat_sbt().replace("% \"0.1.0\"", "% \"0.2.0\"");

        let merged = merge_component_sbt(&current, &new);

        assert!(merged.contains("\"cloud.golem\" %%% \"golem-scala-core\"  % \"0.2.0\""));
        assert!(merged.contains("\"dev.zio\" %%% \"zio\" % \"2.1.0\""));
        assert!(merged.contains(".in(file(\".\"))"));
        assert_eq!(merge_component_sbt(&merged, &new), merged);
        assert_eq!(merge_component_sbt(&flat_sbt(), &flat_sbt()), flat_sbt());
    }

    #[test]
    fn build_sbt_merge_replaces_template_settings_and_keeps_user_settings() {
        let current = "ThisBuild / scalaVersion := \"3.3.0\"\nThisBuild / organization := \"me\"\n";
        let new = "ThisBuild / scalaVersion := \"3.7.1\"\n";

        let merged = merge_build_sbt(current, new);

        assert_eq!(
            merged,
            "ThisBuild / scalaVersion := \"3.7.1\"\nThisBuild / organization := \"me\"\n"
        );
        assert_eq!(merge_build_sbt(&merged, new), merged);
        assert_eq!(
            merge_build_sbt("ThisBuild / organization := \"me\"\n", new),
            "ThisBuild / organization := \"me\"\nThisBuild / scalaVersion := \"3.7.1\"\n"
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

        let merged = merge_plugins_sbt(current, new);

        assert_eq!(
            merged,
            indoc::indoc! {r#"
                addSbtPlugin("cloud.golem" % "golem-scala-sbt" % "0.2.0")
                addSbtPlugin("org.scalameta" % "sbt-scalafmt" % "2.5.2")
                addSbtPlugin("org.scala-js" % "sbt-scalajs" % "1.19.0")
            "#}
        );
        assert_eq!(merge_plugins_sbt(&merged, new), merged);
    }

    #[test]
    fn properties_merge_sets_template_properties_and_keeps_user_properties() {
        let current = "# sbt\nsbt.version=1.10.0\nsbt.color=always\n";
        let new = "sbt.version=1.11.2\n";

        let merged = merge_properties(current, new);

        assert_eq!(merged, "# sbt\nsbt.version=1.11.2\nsbt.color=always\n");
        assert_eq!(merge_properties(&merged, new), merged);
    }

    #[test]
    fn component_dir_errors_when_in_file_missing() {
        let src = "lazy val component_name = project\n  .settings()\n";
        let err = rewrite_component_dir(src, "component_name", "x").unwrap_err();
        assert!(err.to_string().contains(".in(file"));
    }
}

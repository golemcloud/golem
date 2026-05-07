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
    fn component_dir_errors_when_in_file_missing() {
        let src = "lazy val component_name = project\n  .settings()\n";
        let err = rewrite_component_dir(src, "component_name", "x").unwrap_err();
        assert!(err.to_string().contains(".in(file"));
    }
}

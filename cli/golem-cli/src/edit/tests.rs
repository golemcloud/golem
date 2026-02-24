// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::edit::cargo_toml;
use crate::edit::golem_yaml;
use crate::edit::main_rs;
use crate::edit::main_ts;
use crate::edit::package_json;
use crate::edit::tsconfig_json;
use test_r::test;

#[test]
fn package_json_merge_and_collect() {
    let source = r#"{
  // comment
  "name": "demo",
  "dependencies": {
    "foo": "1.0.0"
  },
  "devDependencies": {
    "bar": "2.0.0"
  }
}"#;

    let updated = package_json::merge_dependencies(
        source,
        &[("foo".to_string(), "1.2.0".to_string()), ("baz".to_string(), "3.0.0".to_string())],
        &[("qux".to_string(), "4.0.0".to_string())],
    )
    .unwrap();

    let expected = r#"{
  // comment
  "name": "demo",
  "dependencies": {
    "foo": "1.2.0",
    "baz": "3.0.0"
  },
  "devDependencies": {
    "bar": "2.0.0",
    "qux": "4.0.0"
  }
}"#;

    assert_eq!(updated, expected);

    let collected = package_json::collect_versions(&updated, &["foo", "bar", "baz"]).unwrap();
    assert_eq!(collected.get("foo").unwrap().as_deref(), Some("\"1.2.0\""));
    assert_eq!(collected.get("bar").unwrap().as_deref(), Some("\"2.0.0\""));
    assert_eq!(collected.get("baz").unwrap().as_deref(), Some("\"3.0.0\""));
}

#[test]
fn tsconfig_merge_and_check() {
    let base = r#"{
  // base
  "compilerOptions": {
    "target": "ES2020",
    "module": "CommonJS"
  }
}"#;
    let update = r#"{
  "compilerOptions": {
    "module": "ESNext",
    "strict": true
  },
  "exclude": ["dist"]
}"#;

    let merged = tsconfig_json::merge_with_newer(base, update).unwrap();
    let expected = r#"{
  // base
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "strict": true
  },
  "exclude": ["dist"]
}"#;
    assert_eq!(merged, expected);

    let missing = tsconfig_json::check_required_settings(
        &merged,
        &[
            tsconfig_json::RequiredSetting {
                path: vec!["compilerOptions".to_string(), "module".to_string()],
                expected_literal: Some("\"ESNext\"".to_string()),
            },
            tsconfig_json::RequiredSetting {
                path: vec!["compilerOptions".to_string(), "jsx".to_string()],
                expected_literal: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].path, vec!["compilerOptions".to_string(), "jsx".to_string()]);
}

#[test]
fn cargo_toml_merge_and_check() {
    let source = r#"[package]
name = "demo"

[dependencies]
serde = "1.0"
"#;

    let updated = cargo_toml::merge_dependencies(
        source,
        &[("serde".to_string(), "1.0.200".to_string())],
        &[("anyhow".to_string(), "1.0".to_string())],
    )
    .unwrap();

    assert!(updated.contains("serde = \"1.0.200\""));
    assert!(updated.contains("[dev-dependencies]"));
    assert!(updated.contains("anyhow = \"1.0\""));

    let missing = cargo_toml::check_required_deps(&updated, &["serde", "tokio"]).unwrap();
    assert_eq!(missing, vec!["tokio".to_string()]);
}

#[test]
fn golem_yaml_split_and_edit() {
    let source = r#"app:
  name: demo
components:
  alpha:
    image: alpha
  beta:
    image: beta
other: 1
"#;

    let docs = golem_yaml::split_documents(source, &["app", "components"], &["components"]).unwrap();
    assert_eq!(docs.len(), 4);
    assert!(docs[0].contains("app:"));
    assert!(docs[1].contains("components:"));
    assert!(docs[1].contains("alpha:"));
    assert!(docs[2].contains("beta:"));
    assert!(docs[3].contains("other: 1"));

    let updated = golem_yaml::add_map_entry(source, &["app"], "version", "1").unwrap();
    assert!(updated.contains("version: 1"));

    let updated = golem_yaml::remove_map_entry(&updated, &["components"], "beta").unwrap();
    assert!(!updated.contains("beta:"));

    let updated = golem_yaml::set_scalar(&updated, &["other"], "2").unwrap();
    assert!(updated.contains("other: 2"));
}

#[test]
fn main_ts_reexport() {
    let source = r#"export { a } from "./a";
export { b } from "./b";
"#;
    let updated = main_ts::add_reexport(source, r#"export { c } from "./c";"#).unwrap();
    assert!(updated.contains(r#"export { c } from "./c";"#));
}

#[test]
fn main_rs_import_export() {
    let source = r#"use std::fmt;

fn main() {}
"#;
    let updated = main_rs::add_import_and_export(
        source,
        "use crate::foo::Bar;",
        "pub use crate::foo::Baz;",
    )
    .unwrap();
    assert!(updated.contains("use crate::foo::Bar;"));
    assert!(updated.contains("pub use crate::foo::Baz;"));
}

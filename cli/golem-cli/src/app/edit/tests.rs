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

use crate::app::edit::{
    agents_md, cargo_toml, golem_yaml, main_rs, main_ts, package_json, tsconfig_json,
};
use crate::model::GuestLanguage;
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
        &[
            ("foo".to_string(), "1.2.0".to_string()),
            ("baz".to_string(), "3.0.0".to_string()),
        ],
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
    assert_eq!(
        missing[0].path,
        vec!["compilerOptions".to_string(), "jsx".to_string()]
    );
}

#[test]
fn cargo_toml_merge_and_check() {
    let source = r#"[package]
name = "demo"

[dependencies]
serde = "1.0"
foo = { version = "1.0", features = ["a"] }
workspace_dep = { workspace = true }

[workspace.dependencies]
workspace_shared = "2.0"
"#;

    let updated = cargo_toml::merge_dependencies(
        source,
        &[
            ("serde".to_string(), "1.0.200".to_string()),
            ("foo".to_string(), "1.2+bar,baz".to_string()),
            ("workspace_dep".to_string(), "3.0".to_string()),
            ("new_with_features".to_string(), "0.3+feat1".to_string()),
            ("skip_me".to_string(), "workspace".to_string()),
        ],
        &[("anyhow".to_string(), "1.0".to_string())],
    )
    .unwrap();

    assert!(updated.contains("serde = \"1.0.200\""));
    assert!(updated.contains("[dev-dependencies]"));
    assert!(updated.contains("anyhow = \"1.0\""));

    let doc: toml_edit::DocumentMut = updated.parse().unwrap();
    let deps = doc
        .get("dependencies")
        .and_then(|item| item.as_table())
        .unwrap();
    let foo_item = deps.get("foo").unwrap();
    let foo = table_like(foo_item);
    let features = foo
        .get("features")
        .and_then(|item| item.as_array())
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        foo.get("version").and_then(|item| item.as_str()),
        Some("1.2")
    );
    assert_eq!(features, vec!["a", "bar", "baz"]);
    let new_with_features_item = deps.get("new_with_features").unwrap();
    let new_with_features = table_like(new_with_features_item);
    assert_eq!(
        new_with_features
            .get("version")
            .and_then(|item| item.as_str()),
        Some("0.3")
    );
    let new_features = new_with_features
        .get("features")
        .and_then(|item| item.as_array())
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(new_features, vec!["feat1"]);
    assert!(deps
        .get("workspace_dep")
        .map(table_like)
        .and_then(|table| table.get("workspace"))
        .and_then(|item| item.as_bool())
        .unwrap_or(false));
    assert!(deps.get("skip_me").is_none());

    let missing =
        cargo_toml::check_required_deps(&updated, &["serde", "tokio", "workspace_shared"]).unwrap();
    assert_eq!(missing, vec!["tokio".to_string()]);
}

fn table_like(item: &toml_edit::Item) -> &dyn toml_edit::TableLike {
    if let Some(table) = item.as_table() {
        return table;
    }
    if let Some(table) = item.as_inline_table() {
        return table;
    }
    panic!("Expected table-like item");
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

    let docs =
        golem_yaml::split_documents(source, &["app", "components"], &["components"]).unwrap();
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
fn main_ts_merge_reexports_adds_new() {
    let current = r#"export { a } from "./a";
export { b } from "./b";
"#;
    let update = r#"export { b } from "./b";
export { c } from "./c";
"#;
    let merged = main_ts::merge_reexports(current, update).unwrap();
    let expected = r#"export { a } from "./a";
export { b } from "./b";
export { c } from "./c";
"#;
    assert_eq!(merged, expected);
}

#[test]
fn main_ts_merge_reexports_skips_existing() {
    let current = r#"export { a } from "./a";
export { b } from "./b";
"#;
    let update = r#"export { a } from "./a";
export { b } from "./b";
"#;
    let merged = main_ts::merge_reexports(current, update).unwrap();
    assert_eq!(merged, current);
}

#[test]
fn main_rs_import_export() {
    let source = r#"use std::fmt;

fn main() {}
"#;
    let updated =
        main_rs::add_import_and_export(source, "use crate::foo::Bar;", "pub use crate::foo::Baz;")
            .unwrap();
    assert!(updated.contains("use crate::foo::Bar;"));
    assert!(updated.contains("pub use crate::foo::Baz;"));
}

#[test]
fn main_rs_merge_reexports_adds_new() {
    let current = r#"mod counter_agent;
pub use counter_agent::*;
"#;
    let update = r#"mod human_agent;
pub use human_agent::*;
"#;

    let merged = main_rs::merge_reexports(current, update).unwrap();
    let expected = r#"mod counter_agent;
mod human_agent;

pub use counter_agent::*;
pub use human_agent::*;
"#;
    assert_eq!(merged, expected);
}

#[test]
fn main_rs_merge_reexports_groups_mixed_declarations() {
    let current = r#"pub use counter_agent::*;
mod counter_agent;
"#;
    let update = r#"mod human_agent;
pub use human_agent::*;
"#;

    let merged = main_rs::merge_reexports(current, update).unwrap();
    let expected = r#"mod counter_agent;
mod human_agent;

pub use counter_agent::*;
pub use human_agent::*;
"#;
    assert_eq!(merged, expected);
}

#[test]
fn main_rs_merge_reexports_skips_existing() {
    let current = r#"mod counter_agent;
pub use counter_agent::*;
"#;
    let update = r#"mod counter_agent;
pub use counter_agent::*;
"#;

    let merged = main_rs::merge_reexports(current, update).unwrap();
    assert_eq!(merged, current);
}

#[test]
fn main_ts_preserves_formatting() {
    let source = "export  {a}  from \"./a\";\n\nexport{b}from \"./b\";\n";
    let updated = main_ts::add_reexport(source, "export { c } from \"./c\";").unwrap();
    let expected =
        "export  {a}  from \"./a\";\n\nexport{b}from \"./b\";\nexport { c } from \"./c\";\n";
    assert_eq!(updated, expected);
}

#[test]
fn main_rs_preserves_formatting() {
    let source = "use  std::fmt;\n\nuse crate::old::Thing;\n\nfn main() {}\n";
    let updated =
        main_rs::add_import_and_export(source, "use crate::foo::Bar;", "pub  use crate::foo::Baz;")
            .unwrap();
    let expected = "use  std::fmt;\n\nuse crate::old::Thing;\nuse crate::foo::Bar;\npub  use crate::foo::Baz;\n\nfn main() {}\n";
    assert_eq!(updated, expected);
}

#[test]
fn package_json_preserves_formatting() {
    let source = r#"{  "name" :  "demo",

  "dependencies"  : { "foo" : "1.0.0"  },
  "devDependencies" :{ "bar" :"2.0.0"}
}"#;

    let updated = package_json::merge_dependencies(
        source,
        &[("foo".to_string(), "1.2.0".to_string())],
        &[("bar".to_string(), "2.1.0".to_string())],
    )
    .unwrap();

    let expected = r#"{  "name" :  "demo",

  "dependencies"  : { "foo" : "1.2.0"  },
  "devDependencies" :{ "bar" :"2.1.0"}
}"#;

    assert_eq!(updated, expected);
}

#[test]
fn tsconfig_preserves_formatting() {
    let source = r#"{ "compilerOptions" : { "target" : "ES2020" , "module" : "CommonJS" } }
"#;
    let update = r#"{ "compilerOptions" : { "module" : "ESNext" } }"#;

    let updated = tsconfig_json::merge_with_newer(source, update).unwrap();
    let expected = r#"{ "compilerOptions" : { "target" : "ES2020" , "module" : "ESNext" } }
"#;
    assert_eq!(updated, expected);
}

#[test]
fn cargo_toml_preserves_untouched_formatting() {
    let source = r#"[package]
name = "demo"

[dependencies]
serde  =  "1.0"
untouched  =  "0.1"

foo={version="1.0", features=["a"]}
"#;
    let updated = cargo_toml::merge_dependencies(
        source,
        &[
            ("serde".to_string(), "1.1.0".to_string()),
            ("foo".to_string(), "1.2+bar".to_string()),
        ],
        &[],
    )
    .unwrap();

    assert!(updated.contains("untouched  =  \"0.1\""));
    assert!(updated.contains("serde  = \"1.1.0\""));
}

#[test]
fn cargo_toml_merge_documents_merges_dependency_features() {
    let base = r#"[package]
name = "demo"

[dependencies]
golem-rust = { version = "1.2.3", features = ["export_golem_agentic"] }
serde = { version = "1", features = ["derive"] }
"#;

    let update = r#"[dependencies]
golem-rust = { features = ["chrono"] }
chrono = "0.4.42"
"#;

    let merged = cargo_toml::merge_documents(base, update).unwrap();
    let doc: toml_edit::DocumentMut = merged.parse().unwrap();

    let deps = doc
        .get("dependencies")
        .and_then(|item| item.as_table())
        .unwrap();

    let golem_rust_item = deps.get("golem-rust").unwrap();
    let golem_rust = table_like(golem_rust_item);

    assert_eq!(
        golem_rust.get("version").and_then(|item| item.as_str()),
        Some("1.2.3")
    );

    let features = golem_rust
        .get("features")
        .and_then(|item| item.as_array())
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(features, vec!["export_golem_agentic", "chrono"]);

    assert_eq!(
        deps.get("chrono").and_then(|item| item.as_str()),
        Some("0.4.42")
    );
}

#[test]
fn cargo_toml_merge_documents_keeps_base_sections() {
    let base = r#"[package]
name = "component_name"
version = "0.0.1"

[lib]
crate-type = ["cdylib"]
path = "src/lib.rs"
"#;

    let update = r#"[dependencies]
uuid = { version = "1.15.1", features = ["v4"] }
"#;

    let merged = cargo_toml::merge_documents(base, update).unwrap();
    let doc: toml_edit::DocumentMut = merged.parse().unwrap();

    assert_eq!(
        doc.get("package")
            .and_then(|item| item.get("name"))
            .and_then(|item| item.as_str()),
        Some("component_name")
    );
    assert_eq!(
        doc.get("lib")
            .and_then(|item| item.get("path"))
            .and_then(|item| item.as_str()),
        Some("src/lib.rs")
    );
    assert!(doc
        .get("dependencies")
        .and_then(|item| item.get("uuid"))
        .is_some());
}

#[test]
fn golem_yaml_preserves_formatting() {
    let source = "app :\n  name  :demo\n\nother :  1\n";
    let updated = golem_yaml::set_scalar(source, &["other"], "2").unwrap();
    let expected = "app :\n  name  :demo\n\nother :  2\n";
    assert_eq!(updated, expected);
}

#[test]
fn golem_yaml_merge_documents() {
    let base = r#"app:
  name: demo
  tags:
    - a
    - b
components:
  alpha:
    image: alpha
  beta:
    image: beta
"#;
    let update = r#"app:
  name: demo2
  tags:
    - b
    - c
components:
  beta:
    image: beta2
  gamma:
    image: gamma
other: 1
"#;

    let merged = golem_yaml::merge_documents(base, update).unwrap();
    let merged_value: serde_yaml::Value = serde_yaml::from_str(&merged).unwrap();
    let app = merged_value
        .get("app")
        .and_then(|value| value.as_mapping())
        .unwrap();
    let name = app
        .get(serde_yaml::Value::String("name".to_string()))
        .and_then(|value| value.as_str())
        .unwrap();
    assert_eq!(name, "demo2");
    let tags = app
        .get(serde_yaml::Value::String("tags".to_string()))
        .and_then(|value| value.as_sequence())
        .unwrap();
    let tags = tags
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tags, vec!["a", "b", "c"]);

    let components = merged_value
        .get("components")
        .and_then(|value| value.as_mapping())
        .unwrap();
    let beta = components
        .get(serde_yaml::Value::String("beta".to_string()))
        .and_then(|value| value.as_mapping())
        .unwrap();
    let beta_image = beta
        .get(serde_yaml::Value::String("image".to_string()))
        .and_then(|value| value.as_str())
        .unwrap();
    assert_eq!(beta_image, "beta2");

    assert!(components.contains_key(serde_yaml::Value::String("alpha".to_string())));
    assert!(components.contains_key(serde_yaml::Value::String("gamma".to_string())));
    assert!(merged_value.get("other").is_some());
}

#[test]
fn golem_yaml_merge_keeps_schema_and_app_sections() {
    let base = r#"# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.5.0-dev.1/golem.schema.json
# Schema for vscode-yaml:
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.5.0-dev.1/golem.schema.json

# Field reference: https://learn.golem.cloud/app-manifest#field-reference
# Creating HTTP APIs: https://learn.golem.cloud/invoke/making-custom-apis

app: test-app

environments:
  local:
    server: local
    componentPresets: debug
  cloud:
    server: cloud
    componentPresets: release
components:
  test-app:ts-main:
    templates: ts

    # Component environment variables can reference system environment variables with minijinja syntax:
    #
    #   env:
    #     ENV_VAR_1: "{{ ENV_VAR_1 }}"
    #     RENAMED_VAR_2: "{{ ENV_VAR_2 }}"
    #     COMPOSED_VAR_3: "{{ ENV_VAR_3 }}-{{ ENV_VAR_4}}"
    #
    env:
httpApi:
  deployments:
    local:
    - domain: test-app.localhost:9006
      agents:
        counter-agent: {}

"#;

    let update = r#"# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.5.0-dev.1/golem.schema.json
# Schema for vscode-yaml:
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.5.0-dev.1/golem.schema.json

# Field reference: https://learn.golem.cloud/app-manifest#field-reference
# Creating HTTP APIs: https://learn.golem.cloud/invoke/making-custom-apis

components:
  test-app:ts-main:
    templates: ts

    # Component environment variables can reference system environment variables with minijinja syntax:
    #
    #   env:
    #     ENV_VAR_1: "{{ ENV_VAR_1 }}"
    #     RENAMED_VAR_2: "{{ ENV_VAR_2 }}"
    #     COMPOSED_VAR_3: "{{ ENV_VAR_3 }}-{{ ENV_VAR_4}}"
    #
    env:
"#;

    let merged = golem_yaml::merge_documents(base, update).unwrap();
    assert!(merged.contains("app: test-app"));
    assert!(merged.contains("environments:"));
    assert!(merged.contains("httpApi:"));
    assert!(merged.contains("components:"));
}

#[test]
fn golem_yaml_merge_agents_map_has_no_extra_blank_lines() {
    let base = r#"httpApi:
  deployments:
    local:
    - domain: test-app.localhost:9006
      agents:
        CounterAgent: {}
"#;

    let update = r#"httpApi:
  deployments:
    local:
    - agents:
        WorkflowAgent: {}
        HumanAgent: {}
"#;

    let merged = golem_yaml::merge_documents(base, update).unwrap();

    assert!(merged.contains("WorkflowAgent: {}"));
    assert!(merged.contains("HumanAgent: {}"));
    assert!(!merged.contains("WorkflowAgent: {}\n\n        HumanAgent: {}"));
}

#[test]
fn golem_yaml_merge_normalizes_trailing_empty_lines() {
    let base = "a: 1\n";
    let update = "b: 2\n\n\n";

    let merged = golem_yaml::merge_documents(base, update).unwrap();

    assert!(merged.ends_with('\n'));
    assert!(!merged.ends_with("\n\n"));
}

#[test]
fn agents_md_merge_guides_preserves_user_content() {
    let rust_guide = make_managed_guide(GuestLanguage::Rust, "# Rust guide\nRust body");
    let ts_guide = make_managed_guide(GuestLanguage::TypeScript, "# TS guide\nTS body");

    let current = format!("# My Notes\nPlease keep this\n\n{}", rust_guide);

    let merged = agents_md::merge_guides(&current, &ts_guide).unwrap();

    assert!(merged.contains("# My Notes"));
    assert!(merged.contains("Please keep this"));
    assert!(merged.contains("<!-- golem-managed:guide:rust:start -->"));
    assert!(merged.contains("<!-- golem-managed:guide:ts:start -->"));
    assert!(merged.contains("Rust body"));
    assert!(merged.contains("TS body"));
}

#[test]
fn agents_md_merge_guides_updates_same_language_section() {
    let rust_guide_v1 = make_managed_guide(GuestLanguage::Rust, "# Rust guide\nV1");
    let rust_guide_v2 = make_managed_guide(GuestLanguage::Rust, "# Rust guide\nV2");

    let current = rust_guide_v1;

    let merged = agents_md::merge_guides(&current, &rust_guide_v2).unwrap();

    assert!(merged.contains("V2"));
    assert!(!merged.contains("V1"));
    assert_eq!(
        merged
            .matches("<!-- golem-managed:guide:rust:start -->")
            .count(),
        1
    );
}

fn make_managed_guide(language: GuestLanguage, content: &str) -> String {
    let key = language.id();
    format!(
        "<!-- golem-managed:guide:{key}:start -->\n{content}\n<!-- golem-managed:guide:{key}:end -->\n"
    )
}

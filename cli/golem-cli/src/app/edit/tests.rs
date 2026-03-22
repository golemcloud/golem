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
    agents_md, cargo_toml, gitignore, golem_yaml, json, main_rs, main_ts, package_json,
    tsconfig_json,
};
use crate::model::GuestLanguage;
use proptest::prelude::*;
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeSet;
use test_r::test;

#[test]
fn package_json_merge_and_collect() {
    let source = r#"{
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
fn golem_yaml_merge_replaces_empty_env_with_mapping() {
    let base = r#"app: env-var-substitution

components:
  env-var-substitution:ts-main:
    templates: ts
    env:
httpApi:
  deployments:
    local:
    - domain: env-var-substitution.localhost:9006
      agents:
        CounterAgent: {}
"#;

    let update = r#"components:
  env-var-substitution:ts-main:
    env:
      NORMAL: 'REALLY'
      VERY_CUSTOM_ENV_VAR_SECRET_1: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}'
      VERY_CUSTOM_ENV_VAR_SECRET_2: '{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
      COMPOSED: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
"#;

    let merged = golem_yaml::merge_documents(base, update).unwrap();

    let merged_value: serde_yaml::Value =
        serde_yaml::from_str(&merged).unwrap_or_else(|err| panic!("{err}\nMerged:\n{merged}"));
    let env = merged_value
        .get("components")
        .and_then(|value| value.get("env-var-substitution:ts-main"))
        .and_then(|value| value.get("env"))
        .and_then(|value| value.as_mapping())
        .unwrap();

    assert_eq!(
        env.get(serde_yaml::Value::String("NORMAL".to_string()))
            .and_then(|value| value.as_str()),
        Some("REALLY")
    );
    assert_eq!(
        env.get(serde_yaml::Value::String("COMPOSED".to_string()))
            .and_then(|value| value.as_str()),
        Some("{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}")
    );
    assert!(merged.contains("httpApi:"));
}

#[test]
fn golem_yaml_merge_preserves_base_comments_and_handles_update_comments() {
    let base = r#"# top comment
app:
  # app name comment
  name: demo
components:
  alpha:
    # keep this comment
    image: alpha
"#;

    let update = r#"# update comment should not break parsing
app:
  name: demo-updated
components:
  alpha:
    # update side comment
    image: alpha-updated
  beta:
    image: beta
"#;

    let merged = golem_yaml::merge_documents(base, update).unwrap();

    let merged_value: serde_yaml::Value = serde_yaml::from_str(&merged).unwrap();
    assert_eq!(
        merged_value
            .get("app")
            .and_then(|value| value.get("name"))
            .and_then(|value| value.as_str()),
        Some("demo-updated")
    );
    assert_eq!(
        merged_value
            .get("components")
            .and_then(|value| value.get("alpha"))
            .and_then(|value| value.get("image"))
            .and_then(|value| value.as_str()),
        Some("alpha-updated")
    );
    assert_eq!(
        merged_value
            .get("components")
            .and_then(|value| value.get("beta"))
            .and_then(|value| value.get("image"))
            .and_then(|value| value.as_str()),
        Some("beta")
    );

    assert!(merged.contains("# top comment"));
    assert!(merged.contains("# app name comment"));
    assert!(merged.contains("# keep this comment"));
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

#[test]
fn agents_md_merge_guides_handles_source_order_not_key_order() {
    let ts_guide_v1 = make_managed_guide(GuestLanguage::TypeScript, "# TS guide\nV1");
    let rust_guide_v1 = make_managed_guide(GuestLanguage::Rust, "# Rust guide\nV1");
    let rust_guide_v2 = make_managed_guide(GuestLanguage::Rust, "# Rust guide\nV2");

    let current = format!("{}\n{}", ts_guide_v1, rust_guide_v1);
    let merged = agents_md::merge_guides(&current, &rust_guide_v2).unwrap();

    let ts_idx = merged
        .find("<!-- golem-managed:guide:ts:start -->")
        .unwrap();
    let rust_idx = merged
        .find("<!-- golem-managed:guide:rust:start -->")
        .unwrap();

    assert!(ts_idx < rust_idx);
    assert!(merged.contains("# TS guide\nV1"));
    assert!(merged.contains("# Rust guide\nV2"));
    assert!(!merged.contains("# Rust guide\nV1"));
}

#[test]
fn package_json_rejects_json_comments() {
    let source = r#"{
  // comment
  "dependencies": {
    "foo": "1.0.0"
  }
}"#;

    let result =
        package_json::merge_dependencies(source, &[("foo".to_string(), "1.2.0".to_string())], &[]);

    assert!(result.is_err());
}

#[test]
fn tsconfig_rejects_json_comments() {
    let base = r#"{
  "compilerOptions": {
    "target": "ES2020"
  }
}"#;
    let update = r#"{
  /* comment */
  "compilerOptions": {
    "module": "ESNext"
  }
}"#;

    let result = tsconfig_json::merge_with_newer(base, update);
    assert!(result.is_err());
}

#[test]
fn gitignore_merge_keeps_conflicting_rules_ordered() {
    let source = "target/\n!target/\n";
    let additional = "!target/\ntarget/\n";

    let merged = gitignore::merge(source, additional);
    let expected = "target/\n!target/";
    assert_eq!(merged, expected);
}

#[test]
fn gitignore_merge_sorts_and_dedups_non_conflicting_rules() {
    let source = "b/\na/\na/\n";
    let additional = "c/\nb/\n";

    let merged = gitignore::merge(source, additional);
    let expected = "a/\nb/\nc/";
    assert_eq!(merged, expected);
}

#[test]
fn gitignore_merge_keeps_comments_in_ordered_bucket() {
    let source = "# keep me\nfoo/\n";
    let additional = "bar/\n";

    let merged = gitignore::merge(source, additional);
    let expected = "# keep me\nbar/\nfoo/";
    assert_eq!(merged, expected);
}

#[test]
fn package_json_merge_handles_missing_sections_and_escaped_versions() {
    let source = r#"{
  "name": "demo"
}"#;

    let merged = package_json::merge_dependencies(
        source,
        &[
            ("dep".to_string(), "1.2.3".to_string()),
            (
                "quoted".to_string(),
                "git+ssh://host/repo\\\"branch\"".to_string(),
            ),
        ],
        &[("dev".to_string(), "line1\\nline2".to_string())],
    )
    .unwrap();

    let json: serde_json::Value = serde_json::from_str(&merged).unwrap();
    assert_eq!(
        json["dependencies"]["dep"],
        serde_json::Value::String("1.2.3".to_string())
    );
    assert_eq!(
        json["dependencies"]["quoted"],
        serde_json::Value::String("git+ssh://host/repo\\\"branch\"".to_string())
    );
    assert_eq!(
        json["devDependencies"]["dev"],
        serde_json::Value::String("line1\\nline2".to_string())
    );
}

#[test]
fn json_merge_handles_escaped_keys_and_multiline_values() {
    let base = r#"{
  "shared": {
    "quote\"key": "line1\\nline2",
    "keep": true
  }
}"#;
    let update = r#"{
  "shared": {
    "quote\"key": "updated\\nvalue"
  },
  "added": 1
}"#;

    let merged_source = json::merge_object(base, update).unwrap();
    let merged: serde_json::Value = serde_json::from_str(&merged_source).unwrap();

    assert_eq!(
        merged["shared"]["quote\"key"],
        serde_json::Value::String("updated\\nvalue".to_string())
    );
    assert_eq!(merged["shared"]["keep"], serde_json::Value::Bool(true));
    assert_eq!(merged["added"], serde_json::Value::Number(1.into()));
}

#[test]
fn golem_yaml_merge_handles_literal_and_folded_multiline_scalars() {
    let base = r#"notes: |
  line1
  line2
shared:
  keep: base
"#;
    let update = r#"notes: >
  updated
  lines
shared:
  override: update
"#;

    let merged_source = golem_yaml::merge_documents(base, update).unwrap();
    let merged: serde_yaml::Value = serde_yaml::from_str(&merged_source).unwrap();

    assert_eq!(
        merged.get("notes").and_then(|value| value.as_str()),
        Some("updated lines\n")
    );
    let shared = merged
        .get("shared")
        .and_then(|value| value.as_mapping())
        .unwrap();
    assert_eq!(
        shared
            .get(serde_yaml::Value::String("keep".to_string()))
            .and_then(|value| value.as_str()),
        Some("base")
    );
    assert_eq!(
        shared
            .get(serde_yaml::Value::String("override".to_string()))
            .and_then(|value| value.as_str()),
        Some("update")
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512, .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_json_merge_prefers_update_and_handles_escaping(
        (base_value, update_value, base_style, update_style) in arb_json_merge_case(),
    ) {
        let base = render_json(&base_value, base_style);
        let update = render_json(&update_value, update_style);

        let merged_source = json::merge_object(&base, &update).unwrap();
        let merged: JsonValue = serde_json::from_str(&merged_source).unwrap();
        let expected = merge_json_values(base_value.clone(), update_value.clone());

        prop_assert_eq!(merged.clone(), expected);

        let merged_twice = json::merge_object(&merged_source, &update).unwrap();
        let merged_twice_value: JsonValue = serde_json::from_str(&merged_twice).unwrap();
        prop_assert_eq!(merged_twice_value, merged);
    }

    #[test]
    fn proptest_json_merge_preserves_base_when_update_key_missing(
        base_scalar in arb_json_leaf(),
        update_scalar in arb_json_leaf(),
        include_override_in_update in any::<bool>(),
    ) {
        let base = serde_json::json!({
            "shared": {
                "override": base_scalar.clone(),
                "keep": "base"
            },
            "baseOnly": 1
        });
        let update = if include_override_in_update {
            serde_json::json!({
                "shared": {
                    "override": update_scalar.clone()
                },
                "updateOnly": true
            })
        } else {
            serde_json::json!({
                "shared": {
                    "new": "x"
                },
                "updateOnly": true
            })
        };

        let merged_source = json::merge_object(
            &serde_json::to_string_pretty(&base).unwrap(),
            &serde_json::to_string_pretty(&update).unwrap(),
        )
        .unwrap();
        let merged: serde_json::Value = serde_json::from_str(&merged_source).unwrap();

        if include_override_in_update {
            prop_assert_eq!(merged["shared"]["override"].clone(), update_scalar);
        } else {
            prop_assert_eq!(merged["shared"]["override"].clone(), base_scalar);
            prop_assert_eq!(
                merged["shared"]["new"].clone(),
                serde_json::Value::String("x".to_string())
            );
        }
        prop_assert_eq!(
            merged["shared"]["keep"].clone(),
            serde_json::Value::String("base".to_string())
        );
        prop_assert_eq!(merged["baseOnly"].clone(), serde_json::Value::Number(1.into()));
    }

    #[test]
    fn proptest_json_roundtrip_style_stability(
        (base_value, update_value, base_style, update_style) in arb_json_merge_case(),
        reparsed_style in arb_json_style(),
    ) {
        let base = render_json(&base_value, base_style);
        let update = render_json(&update_value, update_style);

        let merged_once = json::merge_object(&base, &update).unwrap();
        let merged_once_value: JsonValue = serde_json::from_str(&merged_once).unwrap();

        let reparsed_source = render_json(&merged_once_value, reparsed_style);
        let merged_twice = json::merge_object(&reparsed_source, &update).unwrap();
        let merged_twice_value: JsonValue = serde_json::from_str(&merged_twice).unwrap();

        prop_assert_eq!(merged_twice_value, merged_once_value);
    }

    #[test]
    fn proptest_yaml_merge_handles_missing_keys_multiline_and_sequences(
        case in arb_yaml_merge_case(),
    ) {
        let (base_source, update_source, expected_override, expected_multiline, expected_list_union, expected_new_key_present) = case;
        let merged_source = golem_yaml::merge_documents(&base_source, &update_source).unwrap();

        let merged: serde_yaml::Value = serde_yaml::from_str(&merged_source).unwrap();

        let shared = merged.get("shared").and_then(|value| value.as_mapping()).unwrap();
        let override_value = shared
            .get(serde_yaml::Value::String("override".to_string()))
            .and_then(|value| value.as_str())
            .unwrap();
        prop_assert_eq!(override_value, expected_override);

        let notes = merged.get("notes").and_then(|value| value.as_str()).unwrap();
        prop_assert_eq!(notes, expected_multiline);

        let merged_items = merged
            .get("items")
            .and_then(|value| value.as_sequence())
            .unwrap()
            .iter()
            .map(yaml_scalar_to_string)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        prop_assert_eq!(merged_items, expected_list_union);

        let has_new_key = merged.get("newKey").is_some();
        prop_assert_eq!(has_new_key, expected_new_key_present);

        prop_assert!(merged_source.ends_with('\n'));
        prop_assert!(!merged_source.ends_with("\n\n"));
    }

    #[test]
    fn proptest_yaml_merge_preserves_base_comments(
        (top_comment, nested_comment, base_name, update_name, add_new_key) in arb_yaml_comment_merge_case(),
    ) {
        let base = format!(
            "# {top_comment}\napp:\n  # {nested_comment}\n  name: {base_name}\n"
        );
        let update = if add_new_key {
            format!(
                "# update comment\napp:\n  name: {update_name}\nextra: added\n"
            )
        } else {
            format!("# update comment\napp:\n  name: {update_name}\n")
        };

        let merged_source = golem_yaml::merge_documents(&base, &update).unwrap();
        let merged: serde_yaml::Value = serde_yaml::from_str(&merged_source).unwrap();
        let top_comment_line = format!("# {}", top_comment);
        let nested_comment_line = format!("# {}", nested_comment);

        prop_assert!(merged_source.contains(&top_comment_line));
        prop_assert!(merged_source.contains(&nested_comment_line));
        prop_assert_eq!(
            merged
                .get("app")
                .and_then(|value| value.get("name"))
                .and_then(|value| value.as_str()),
            Some(update_name.as_str())
        );

        if add_new_key {
            prop_assert_eq!(
                merged.get("extra").and_then(|value| value.as_str()),
                Some("added")
            );
        }
    }

    #[test]
    fn proptest_yaml_roundtrip_style_stability(
        case in arb_yaml_merge_case(),
        rescale in prop_oneof![Just(1usize), Just(2usize)],
    ) {
        let (base_source, update_source, _, _, _, _) = case;

        let merged_once = golem_yaml::merge_documents(&base_source, &update_source).unwrap();
        let merged_once_value: serde_yaml::Value = serde_yaml::from_str(&merged_once).unwrap();

        let reparsed = serde_yaml::to_string(&merged_once_value).unwrap();
        let reparsed_scaled = scale_yaml_indentation(&reparsed, rescale);

        let merged_twice = golem_yaml::merge_documents(&reparsed_scaled, &update_source).unwrap();
        let merged_twice_value: serde_yaml::Value = serde_yaml::from_str(&merged_twice).unwrap();

        prop_assert_eq!(merged_twice_value, merged_once_value);
    }

    #[test]
    fn proptest_cargo_toml_merge_merges_features_and_preserves_package(
        case in arb_cargo_merge_case(),
    ) {
        let (base_source, update_source, shared_dep_name, expected_features, expected_version) = case;
        let merged_source = cargo_toml::merge_documents(&base_source, &update_source).unwrap();
        let merged_doc: toml_edit::DocumentMut = merged_source.parse().unwrap();

        let package_name = merged_doc
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(|item| item.as_str())
            .unwrap();
        prop_assert_eq!(package_name, "demo");

        let shared_item = merged_doc
            .get("dependencies")
            .and_then(|item| item.get(&shared_dep_name))
            .unwrap();
        let shared_table = table_like(shared_item);
        let version = shared_table
            .get("version")
            .and_then(|item| item.as_str())
            .unwrap();
        prop_assert_eq!(version, expected_version);

        let features = shared_table
            .get("features")
            .and_then(|item| item.as_array())
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        prop_assert_eq!(features, expected_features);

        let merged_twice = cargo_toml::merge_documents(&merged_source, &update_source).unwrap();
        let merged_twice_doc: toml_edit::DocumentMut = merged_twice.parse().unwrap();
        prop_assert_eq!(merged_doc.to_string(), merged_twice_doc.to_string());
    }

    #[test]
    fn proptest_cargo_toml_roundtrip_style_stability(
        case in arb_cargo_merge_case(),
    ) {
        let (base_source, update_source, _, _, _) = case;

        let merged_once = cargo_toml::merge_documents(&base_source, &update_source).unwrap();
        let merged_once_doc: toml_edit::DocumentMut = merged_once.parse().unwrap();

        let reparsed_source = merged_once_doc.to_string();
        let merged_twice = cargo_toml::merge_documents(&reparsed_source, &update_source).unwrap();
        let merged_twice_doc: toml_edit::DocumentMut = merged_twice.parse().unwrap();

        prop_assert_eq!(merged_once_doc.to_string(), merged_twice_doc.to_string());
    }

    #[test]
    fn proptest_main_ts_merge_reexports_is_union_and_idempotent(
        (current_source, update_source, expected) in arb_ts_reexport_case(),
    ) {
        let merged = main_ts::merge_reexports(&current_source, &update_source).unwrap();
        main_ts::validate(&merged).unwrap();

        let merged_set = collect_trimmed_prefixed_lines(&merged, "export ");
        prop_assert_eq!(merged_set, expected);

        let merged_twice = main_ts::merge_reexports(&merged, &update_source).unwrap();
        prop_assert_eq!(merged_twice, merged);
    }

    #[test]
    fn proptest_main_rs_merge_reexports_is_union_and_idempotent(
        (current_source, update_source, expected) in arb_rust_reexport_case(),
    ) {
        let merged = main_rs::merge_reexports(&current_source, &update_source).unwrap();
        main_rs::validate(&merged).unwrap();

        let merged_set = collect_trimmed_prefixed_lines(&merged, "mod ")
            .into_iter()
            .chain(collect_trimmed_prefixed_lines(&merged, "pub use "))
            .collect::<BTreeSet<_>>();
        prop_assert_eq!(merged_set, expected);

        let merged_twice = main_rs::merge_reexports(&merged, &update_source).unwrap();
        prop_assert_eq!(merged_twice, merged);
    }

    #[test]
    fn proptest_agents_md_merge_updates_managed_sections_and_keeps_user_text(
        (user_prefix, user_suffix, rust_body, ts_body, replacement_body) in arb_agents_case(),
    ) {
        let rust_guide = make_managed_guide(GuestLanguage::Rust, &rust_body);
        let ts_guide = make_managed_guide(GuestLanguage::TypeScript, &ts_body);
        let current = format!("{user_prefix}\n\n{rust_guide}\n{ts_guide}\n{user_suffix}");

        let replacement = make_managed_guide(GuestLanguage::Rust, &replacement_body);
        let merged = agents_md::merge_guides(&current, &replacement).unwrap();
        let normalized_replacement = replacement_body.trim_end();
        let normalized_old_rust = rust_body.trim_end();
        let normalized_ts_body = ts_body.trim_end();

        prop_assert!(merged.contains(&user_prefix));
        prop_assert!(merged.contains(&user_suffix));
        prop_assert!(merged.contains(normalized_replacement));
        if normalized_replacement != normalized_old_rust
            && !normalized_replacement.contains(normalized_old_rust)
        {
            prop_assert!(!merged.contains(normalized_old_rust));
        }
        prop_assert!(merged.contains(normalized_ts_body));
        prop_assert_eq!(merged.matches("<!-- golem-managed:guide:rust:start -->").count(), 1);
        prop_assert_eq!(merged.matches("<!-- golem-managed:guide:ts:start -->").count(), 1);
    }

    #[test]
    fn proptest_gitignore_merge_dedups_and_is_idempotent(
        (source_lines, additional_lines) in arb_gitignore_case(),
    ) {
        let source = source_lines.join("\n");
        let additional = additional_lines.join("\n");
        let merged = gitignore::merge(&source, &additional);

        let merged_lines = merged.lines().map(ToString::to_string).collect::<Vec<_>>();
        let unique_count = merged_lines.iter().collect::<BTreeSet<_>>().len();
        prop_assert_eq!(merged_lines.len(), unique_count);

        let merged_twice = gitignore::merge(&merged, &additional);
        prop_assert_eq!(merged_twice, merged);
    }
}

#[derive(Debug, Clone, Copy)]
enum JsonStyle {
    Compact,
    Pretty2,
    Pretty4,
    CompactTrailingNewline,
    PrettyTrailingNewline,
}

fn arb_json_style() -> impl Strategy<Value = JsonStyle> {
    prop_oneof![
        Just(JsonStyle::Compact),
        Just(JsonStyle::Pretty2),
        Just(JsonStyle::Pretty4),
        Just(JsonStyle::CompactTrailingNewline),
        Just(JsonStyle::PrettyTrailingNewline),
    ]
}

fn arb_text() -> impl Strategy<Value = String> {
    // String pool intentionally includes punctuation and escaped payloads
    // to stress parsers and merge replacements.
    prop_oneof![
        Just("simple".to_string()),
        Just("line1\\nline2".to_string()),
        Just("value with : and #".to_string()),
        Just("quote \" and slash \\\\".to_string()),
        "[A-Za-z0-9_:\\- ]{0,20}".prop_map(|value| value),
    ]
}

fn arb_json_leaf() -> impl Strategy<Value = JsonValue> {
    // Scalar JSON leaves used to verify overwrite/preserve semantics
    // across heterogeneous scalar types.
    prop_oneof![
        arb_text().prop_map(JsonValue::String),
        any::<bool>().prop_map(JsonValue::Bool),
        (-1000i64..=1000i64).prop_map(|value| JsonValue::Number(value.into())),
        Just(JsonValue::Null),
    ]
}

fn arb_json_merge_case() -> impl Strategy<Value = (JsonValue, JsonValue, JsonStyle, JsonStyle)> {
    // Generates overlapping JSON objects where both sides touch nested keys,
    // arrays, and optional side-specific keys, then renders with style variance.
    (
        arb_text(),
        arb_text(),
        arb_text(),
        arb_text(),
        prop::collection::vec(arb_text(), 1..5),
        prop::collection::vec(arb_text(), 1..5),
        prop::option::of(arb_text()),
        prop::option::of(arb_text()),
        arb_json_style(),
        arb_json_style(),
    )
        .prop_map(
            |(
                base_override,
                update_override,
                base_note,
                update_note,
                base_items,
                update_items,
                base_new,
                update_new,
                base_style,
                update_style,
            )| {
                let mut base_map = JsonMap::new();
                base_map.insert("shared".to_string(), {
                    let mut shared = JsonMap::new();
                    shared.insert("override".to_string(), JsonValue::String(base_override));
                    shared.insert("note".to_string(), JsonValue::String(base_note));
                    JsonValue::Object(shared)
                });
                base_map.insert(
                    "items".to_string(),
                    JsonValue::Array(base_items.into_iter().map(JsonValue::String).collect()),
                );
                if let Some(value) = base_new {
                    base_map.insert("baseOnly".to_string(), JsonValue::String(value));
                }

                let mut update_map = JsonMap::new();
                update_map.insert("shared".to_string(), {
                    let mut shared = JsonMap::new();
                    shared.insert("override".to_string(), JsonValue::String(update_override));
                    shared.insert("note".to_string(), JsonValue::String(update_note));
                    JsonValue::Object(shared)
                });
                update_map.insert(
                    "items".to_string(),
                    JsonValue::Array(update_items.into_iter().map(JsonValue::String).collect()),
                );
                if let Some(value) = update_new {
                    update_map.insert("updateOnly".to_string(), JsonValue::String(value));
                }

                (
                    JsonValue::Object(base_map),
                    JsonValue::Object(update_map),
                    base_style,
                    update_style,
                )
            },
        )
}

fn render_json(value: &JsonValue, style: JsonStyle) -> String {
    let mut source = match style {
        JsonStyle::Compact | JsonStyle::CompactTrailingNewline => {
            serde_json::to_string(value).unwrap()
        }
        JsonStyle::Pretty2 | JsonStyle::PrettyTrailingNewline => {
            serde_json::to_string_pretty(value).unwrap()
        }
        JsonStyle::Pretty4 => {
            let mut bytes = Vec::new();
            let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
            let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
            value.serialize(&mut serializer).unwrap();
            String::from_utf8(bytes).unwrap()
        }
    };

    if matches!(
        style,
        JsonStyle::CompactTrailingNewline | JsonStyle::PrettyTrailingNewline
    ) {
        source.push('\n');
    }

    source
}

fn merge_json_values(base: JsonValue, update: JsonValue) -> JsonValue {
    match (base, update) {
        (JsonValue::Object(mut base_obj), JsonValue::Object(update_obj)) => {
            for (key, update_value) in update_obj {
                let merged = if let Some(base_value) = base_obj.remove(&key) {
                    merge_json_values(base_value, update_value)
                } else {
                    update_value
                };
                base_obj.insert(key, merged);
            }
            JsonValue::Object(base_obj)
        }
        (_, update) => update,
    }
}

fn arb_yaml_merge_case(
) -> impl Strategy<Value = (String, String, String, String, Vec<String>, bool)> {
    // Produces YAML mappings with overlapping keys, list merges, optional
    // missing/new keys, and indentation scaling to stress formatting paths.
    (
        arb_text(),
        arb_text(),
        arb_text(),
        arb_text(),
        prop::collection::vec(arb_text(), 1..5),
        prop::collection::vec(arb_text(), 1..5),
        prop::option::of(arb_text()),
        any::<bool>(),
        prop_oneof![Just(1usize), Just(2usize)],
        prop_oneof![Just(1usize), Just(2usize)],
    )
        .prop_map(
            |(
                base_override,
                update_override,
                base_notes,
                update_notes,
                base_items,
                update_items,
                update_new_key,
                include_base_only,
                base_indent_scale,
                update_indent_scale,
            )| {
                let mut base = serde_yaml::Mapping::new();
                let mut base_shared = serde_yaml::Mapping::new();
                base_shared.insert(
                    serde_yaml::Value::String("override".to_string()),
                    serde_yaml::Value::String(base_override),
                );
                base_shared.insert(
                    serde_yaml::Value::String("keep".to_string()),
                    serde_yaml::Value::String("base-keep".to_string()),
                );
                base.insert(
                    serde_yaml::Value::String("shared".to_string()),
                    serde_yaml::Value::Mapping(base_shared),
                );
                base.insert(
                    serde_yaml::Value::String("notes".to_string()),
                    serde_yaml::Value::String(base_notes),
                );
                base.insert(
                    serde_yaml::Value::String("items".to_string()),
                    serde_yaml::Value::Sequence(
                        base_items
                            .iter()
                            .cloned()
                            .map(serde_yaml::Value::String)
                            .collect(),
                    ),
                );
                if include_base_only {
                    base.insert(
                        serde_yaml::Value::String("baseOnly".to_string()),
                        serde_yaml::Value::String("present".to_string()),
                    );
                }

                let mut update = serde_yaml::Mapping::new();
                let mut update_shared = serde_yaml::Mapping::new();
                update_shared.insert(
                    serde_yaml::Value::String("override".to_string()),
                    serde_yaml::Value::String(update_override.clone()),
                );
                update.insert(
                    serde_yaml::Value::String("shared".to_string()),
                    serde_yaml::Value::Mapping(update_shared),
                );
                update.insert(
                    serde_yaml::Value::String("notes".to_string()),
                    serde_yaml::Value::String(update_notes.clone()),
                );
                update.insert(
                    serde_yaml::Value::String("items".to_string()),
                    serde_yaml::Value::Sequence(
                        update_items
                            .iter()
                            .cloned()
                            .map(serde_yaml::Value::String)
                            .collect(),
                    ),
                );
                if let Some(new_value) = update_new_key.clone() {
                    update.insert(
                        serde_yaml::Value::String("newKey".to_string()),
                        serde_yaml::Value::String(new_value),
                    );
                }

                let mut list_union = base_items.clone();
                for item in update_items {
                    if !list_union.contains(&item) {
                        list_union.push(item);
                    }
                }

                (
                    scale_yaml_indentation(
                        &serde_yaml::to_string(&serde_yaml::Value::Mapping(base)).unwrap(),
                        base_indent_scale,
                    ),
                    scale_yaml_indentation(
                        &serde_yaml::to_string(&serde_yaml::Value::Mapping(update)).unwrap(),
                        update_indent_scale,
                    ),
                    update_override,
                    update_notes,
                    list_union,
                    update_new_key.is_some(),
                )
            },
        )
}

fn yaml_scalar_to_string(value: &serde_yaml::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value.as_i64().map(|number| number.to_string())
}

fn scale_yaml_indentation(source: &str, scale: usize) -> String {
    if scale == 1 {
        return source.to_string();
    }

    let mut out = String::new();
    for (idx, line) in source.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let leading = line.chars().take_while(|ch| *ch == ' ').count();
        out.push_str(&" ".repeat(leading * scale));
        out.push_str(&line[leading..]);
    }
    out.push('\n');
    out
}

fn arb_cargo_merge_case() -> impl Strategy<Value = (String, String, String, Vec<String>, String)> {
    // Produces TOML dependency merges with shared deps, base-only deps,
    // optional spacing style differences, and feature overlap.
    (
        "[a-z][a-z0-9_-]{0,8}",
        "[a-z][a-z0-9_-]{0,8}",
        "0\\.[0-9]{1,2}\\.[0-9]{1,2}",
        "0\\.[0-9]{1,2}\\.[0-9]{1,2}",
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 0..3),
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 0..3),
        any::<bool>(),
    )
        .prop_map(
            |(
                shared_dep,
                mut base_only_dep,
                base_version,
                update_version,
                base_features,
                update_features,
                use_spaces,
            )| {
                if shared_dep == base_only_dep {
                    base_only_dep.push_str("_base");
                }

                let spacing = if use_spaces { " = " } else { "=" };
                let mut base_source = String::new();
                base_source.push_str("[package]\n");
                base_source.push_str("name = \"demo\"\n");
                base_source.push_str("description = \"line1\\nline2\"\n\n");
                base_source.push_str("[dependencies]\n");
                base_source.push_str(&format!(
                    "{shared_dep}{spacing}{{ version = \"{base_version}\", features = [{}] }}\n",
                    format_feature_list(&base_features)
                ));
                base_source.push_str(&format!(
                    "{base_only_dep}{spacing}{{ version = \"1.0.0\", features = [\"base\"] }}\n"
                ));

                let mut update_source = String::new();
                update_source.push_str("[dependencies]\n");
                update_source.push_str(&format!(
                    "{shared_dep}{spacing}{{ version = \"{update_version}\", features = [{}] }}\n",
                    format_feature_list(&update_features)
                ));
                update_source.push_str("new_dep = \"0.1.0\"\n");

                let mut expected_features = Vec::new();
                for feature in base_features.into_iter().chain(update_features) {
                    if !expected_features.contains(&feature) {
                        expected_features.push(feature);
                    }
                }

                (
                    base_source,
                    update_source,
                    shared_dep,
                    expected_features,
                    update_version,
                )
            },
        )
}

fn format_feature_list(features: &[String]) -> String {
    features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn arb_ts_reexport_case() -> impl Strategy<Value = (String, String, BTreeSet<String>)> {
    // Produces TS export statements with overlap and spacing/newline variance
    // to validate stable union + idempotence behavior.
    (
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 1..5),
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 1..5),
        any::<bool>(),
    )
        .prop_map(|(current_names, update_names, extra_blank_line)| {
            let current_lines = current_names
                .iter()
                .map(|name| format!("export {{ {name} }} from \"./{name}\";"))
                .collect::<Vec<_>>();
            let update_lines = update_names
                .iter()
                .map(|name| format!("export {{ {name} }} from \"./{name}\";"))
                .collect::<Vec<_>>();

            let mut current_source = current_lines.join("\n");
            if extra_blank_line {
                current_source.push_str("\n\n");
            } else {
                current_source.push('\n');
            }

            let mut update_source = update_lines.join("\n");
            update_source.push('\n');

            let expected = current_lines
                .into_iter()
                .chain(update_lines)
                .collect::<BTreeSet<_>>();

            (current_source, update_source, expected)
        })
}

fn arb_rust_reexport_case() -> impl Strategy<Value = (String, String, BTreeSet<String>)> {
    // Produces Rust mod/pub use declaration sets for union/idempotence checks
    // while keeping full snippets syntactically valid.
    (
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 1..4),
        prop::collection::vec("[a-z][a-z0-9_]{0,8}", 1..4),
    )
        .prop_map(|(current_mods, update_mods)| {
            let current_decl = current_mods
                .iter()
                .flat_map(|module| vec![format!("mod {module};"), format!("pub use {module}::*;")])
                .collect::<Vec<_>>();
            let update_decl = update_mods
                .iter()
                .flat_map(|module| vec![format!("mod {module};"), format!("pub use {module}::*;")])
                .collect::<Vec<_>>();

            let current_source = format!("{}\n\nfn main() {{}}\n", current_decl.join("\n"));
            let update_source = format!("{}\n", update_decl.join("\n"));

            let expected = current_decl
                .into_iter()
                .chain(update_decl)
                .collect::<BTreeSet<_>>();

            (current_source, update_source, expected)
        })
}

fn collect_trimmed_prefixed_lines(source: &str, prefix: &str) -> BTreeSet<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(prefix) && line.ends_with(';'))
        .map(ToString::to_string)
        .collect()
}

fn arb_agents_case() -> impl Strategy<Value = (String, String, String, String, String)> {
    // Generates managed guide bodies and user-owned prefix/suffix text to test
    // replacement of same-language blocks while preserving user content.
    (arb_text(), arb_text(), arb_text(), arb_text(), arb_text()).prop_map(
        |(prefix, suffix, rust_body, ts_body, replacement)| {
            (
                format!("# User notes\n{prefix}"),
                format!("## Footer\n{suffix}"),
                format!("# Rust guide\n{rust_body}"),
                format!("# TS guide\n{ts_body}"),
                format!("# Rust guide\n{replacement}"),
            )
        },
    )
}

fn arb_gitignore_case() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    // Generates overlapping positive/negative ignore rules plus optional
    // comments/blank lines to exercise ordering + dedup behavior.
    (
        prop::collection::vec("[a-z][a-z0-9_-]{0,8}", 1..6),
        prop::collection::vec("[a-z][a-z0-9_-]{0,8}", 1..6),
        any::<bool>(),
    )
        .prop_map(|(left, right, add_comment)| {
            let mut source = left
                .into_iter()
                .enumerate()
                .map(|(idx, pattern)| {
                    if idx % 2 == 0 {
                        pattern
                    } else {
                        format!("!{pattern}")
                    }
                })
                .collect::<Vec<_>>();
            let mut additional = right
                .into_iter()
                .enumerate()
                .map(|(idx, pattern)| {
                    if idx % 2 == 0 {
                        pattern
                    } else {
                        format!("!{pattern}")
                    }
                })
                .collect::<Vec<_>>();
            if add_comment {
                source.push("# keep this comment".to_string());
                additional.push("".to_string());
            }
            (source, additional)
        })
}

fn arb_yaml_comment_merge_case() -> impl Strategy<Value = (String, String, String, String, bool)> {
    // Produces safe comment and scalar tokens (no newlines/colon separators)
    // for YAML snippets that intentionally include comments on both sides.
    (
        "[A-Za-z0-9 _-]{1,20}",
        "[A-Za-z0-9 _-]{1,20}",
        "[a-z][a-z0-9_-]{0,12}",
        "[a-z][a-z0-9_-]{0,12}",
        any::<bool>(),
    )
}

fn make_managed_guide(language: GuestLanguage, content: &str) -> String {
    let key = language.id();
    format!(
        "<!-- golem-managed:guide:{key}:start -->\n{content}\n<!-- golem-managed:guide:{key}:end -->\n"
    )
}

// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::stub::StubDefinition;
use anyhow::{anyhow, bail};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting,
};
use golem_wasm_rpc::WASM_RPC_VERSION;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use toml::Value;

#[derive(Serialize, Default)]
struct MetadataRoot {
    component: Option<ComponentMetadata>,
}

#[derive(Serialize)]
struct ComponentMetadata {
    package: String,
    target: ComponentTarget,
}

#[derive(Serialize)]
struct ComponentTarget {
    world: String,
    path: String,
    dependencies: HashMap<String, WitDependency>,
}

#[derive(Serialize)]
struct WitDependency {
    path: String,
}

pub fn generate_cargo_toml(def: &StubDefinition) -> anyhow::Result<()> {
    let mut manifest = Manifest::default();

    let mut wit_dependencies = HashMap::new();

    wit_dependencies.insert(
        def.root_package_name.to_string(),
        WitDependency {
            path: format!(
                "wit/deps/{}_{}",
                def.root_package_name.namespace, def.root_package_name.name
            ),
        },
    );
    wit_dependencies.insert(
        "golem:rpc".to_string(),
        WitDependency {
            path: "wit/deps/wasm-rpc".to_string(),
        },
    );
    for dep in &def.unresolved_deps {
        let mut dirs = HashSet::new();
        for source in dep.source_files() {
            let relative = source.strip_prefix(&def.source_wit_root)?;
            let dir = relative
                .parent()
                .ok_or(anyhow!("Package source {source:?} has no parent directory"))?;
            dirs.insert(dir);
        }

        if dirs.len() != 1 {
            bail!("Package {} has multiple source directories", dep.name);
        }

        wit_dependencies.insert(
            format!("{}:{}", dep.name.namespace, dep.name.name),
            WitDependency {
                path: format!("wit/{}", dirs.iter().next().unwrap().to_str().unwrap()),
            },
        );
    }

    let metadata = MetadataRoot {
        component: Some(ComponentMetadata {
            package: format!(
                "{}:{}",
                def.root_package_name.namespace, def.root_package_name.name
            ),
            target: ComponentTarget {
                world: def.target_world_name()?,
                path: "wit".to_string(),
                dependencies: wit_dependencies,
            },
        }),
    };

    let mut package = cargo_toml::Package::new(def.target_crate_name()?, &def.stub_crate_version);
    package.edition = Inheritable::Set(Edition::E2021);
    package.metadata = Some(metadata);
    manifest.package = Some(package);

    let lib = cargo_toml::Product {
        path: Some("src/lib.rs".to_string()),
        crate_type: vec!["cdylib".to_string()],
        ..Default::default()
    };
    manifest.lib = Some(lib);

    manifest.profile = Profiles {
        release: Some(Profile {
            lto: Some(LtoSetting::Fat),
            opt_level: Some(Value::String("s".to_string())),
            debug: None,
            split_debuginfo: None,
            rpath: None,
            debug_assertions: None,
            codegen_units: None,
            panic: None,
            incremental: None,
            overflow_checks: None,
            strip: Some(StripSetting::Symbols),
            package: BTreeMap::new(),
            build_override: None,
            inherits: None,
        }),
        ..Default::default()
    };

    let dep_wit_bindgen = Dependency::Detailed(Box::new(DependencyDetail {
        version: Some("0.17.0".to_string()),
        default_features: false,
        features: vec!["realloc".to_string()],
        ..Default::default()
    }));

    let dep_golem_wasm_rpc = Dependency::Detailed(Box::new(DependencyDetail {
        version: if def.wasm_rpc_path_override.is_none() {
            Some(WASM_RPC_VERSION.to_string())
        } else {
            None
        },
        path: def.wasm_rpc_path_override.clone(),
        default_features: false,
        features: vec!["stub".to_string()],
        ..Default::default()
    }));

    let mut deps = DepsSet::new();
    deps.insert("wit-bindgen".to_string(), dep_wit_bindgen);
    deps.insert("golem-wasm-rpc".to_string(), dep_golem_wasm_rpc);
    manifest.dependencies = deps;

    let cargo_toml = toml::to_string(&manifest)?;

    println!(
        "Generating Cargo.toml to {}",
        def.target_cargo_path().to_string_lossy()
    );
    fs::write(def.target_cargo_path(), cargo_toml)?;
    Ok(())
}

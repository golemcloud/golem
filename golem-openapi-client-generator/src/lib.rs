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

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::result;

use openapiv3::OpenAPI;

pub use merger::merge_all_openapi_specs;

use crate::rust::lib_gen::{Module, ModuleDef, ModuleName};
use crate::rust::model_gen::RefCache;

pub(crate) mod merger;
pub(crate) mod printer;
mod rust;
mod toml;

#[derive(Debug, Clone)]
pub enum Error {
    Unexpected { message: String },
    Unimplemented { message: String },
}

impl Error {
    pub fn unexpected<S: Into<String>>(message: S) -> Error {
        Error::Unexpected {
            message: message.into(),
        }
    }
    pub fn unimplemented<S: Into<String>>(message: S) -> Error {
        Error::Unimplemented {
            message: message.into(),
        }
    }

    pub fn extend<S: Into<String>>(&self, s: S) -> Error {
        match self {
            Error::Unexpected { message } => Error::Unexpected {
                message: format!("{} {}", s.into(), message.clone()),
            },
            Error::Unimplemented { message } => Error::Unimplemented {
                message: format!("{} {}", s.into(), message.clone()),
            },
        }
    }
}

pub type Result<T> = result::Result<T, Error>;

#[allow(clippy::too_many_arguments)]
pub fn gen(
    openapi_specs: Vec<OpenAPI>,
    target: &Path,
    name: &str,
    version: &str,
    overwrite_cargo: bool,
    disable_clippy: bool,
    mapping: &[(&str, &str)],
    ignored_paths: &[&str],
) -> Result<()> {
    let mapping: HashMap<&str, &str> = HashMap::from_iter(mapping.iter().cloned());

    let open_api = merge_all_openapi_specs(openapi_specs)?;

    let src = target.join("src");
    let api = src.join("api");
    let model = src.join("model");

    std::fs::create_dir_all(&api).unwrap();
    std::fs::create_dir_all(&model).unwrap();

    let context = rust::context_gen::context_gen(&open_api)?;
    std::fs::write(src.join(context.def.name.file_name()), &context.code).unwrap();

    let mut ref_cache = RefCache::new();

    let modules: Result<Vec<Module>> = open_api
        .tags
        .iter()
        .map(|tag| {
            rust::client_gen::client_gen(
                &open_api,
                Some(tag.clone()),
                &mut ref_cache,
                ignored_paths,
            )
        })
        .collect();

    let mut api_module_defs = Vec::new();

    for module in modules? {
        std::fs::write(api.join(module.def.name.file_name()), module.code).unwrap();
        api_module_defs.push(module.def.clone())
    }

    let mut known_refs = RefCache::new();
    let mut models = Vec::new();

    let multipart_field_file = rust::model_gen::multipart_field_module()?;
    std::fs::write(
        model.join(multipart_field_file.def.name.file_name()),
        multipart_field_file.code,
    )
    .unwrap();
    models.push(multipart_field_file.def);

    while !ref_cache.is_empty() {
        let mut next_ref_cache = RefCache::new();

        for ref_str in ref_cache.refs {
            if !known_refs.refs.contains(&ref_str) {
                let model_file =
                    rust::model_gen::model_gen(&ref_str, &open_api, &mapping, &mut next_ref_cache)?;
                std::fs::write(model.join(model_file.def.name.file_name()), model_file.code)
                    .unwrap();
                models.push(model_file.def);
                known_refs.add(ref_str);
            }
        }

        let mut unknown_ref_cache = RefCache::new();

        for ref_str in next_ref_cache.refs {
            if !known_refs.refs.contains(&ref_str) {
                unknown_ref_cache.add(ref_str)
            }
        }

        ref_cache = unknown_ref_cache;
    }

    std::fs::write(
        src.join("api.rs"),
        rust::lib_gen::lib_gen("crate::api", &api_module_defs, disable_clippy),
    )
    .unwrap();

    std::fs::write(
        src.join("model.rs"),
        rust::lib_gen::lib_gen("crate::model", &models, disable_clippy),
    )
    .unwrap();

    let errors = rust::error_gen::error_gen();
    std::fs::write(src.join(errors.def.name.file_name()), &errors.code).unwrap();

    let module_defs = vec![
        context.def,
        ModuleDef::new(ModuleName::new_pub("api")),
        ModuleDef::new(ModuleName::new_pub("model")),
        errors.def,
    ];

    let lib = rust::lib_gen::lib_gen("crate", &module_defs, disable_clippy);
    std::fs::write(src.join("lib.rs"), lib).unwrap();

    if overwrite_cargo {
        let cargo = toml::cargo::gen(name, version);
        std::fs::write(target.join("Cargo.toml"), cargo).unwrap();
    }

    Ok(())
}

pub fn parse_openapi_specs(
    spec: &[PathBuf],
) -> std::result::Result<Vec<OpenAPI>, Box<dyn std::error::Error>> {
    spec.iter()
        .map(|spec_path| {
            let file = File::open(spec_path)?;
            let reader = BufReader::new(file);
            let openapi: OpenAPI = serde_yaml::from_reader(reader)?;
            Ok(openapi)
        })
        .collect()
}

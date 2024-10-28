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

use crate::log::{log_action, log_validated_action_result};
use crate::model::application::{
    Application, ComponentBuildProperties, DEFAULT_CONFIG_FILE_NAME, OAM_COMPONENT_TYPE_WASM_BUILD,
};
use crate::model::oam;
use crate::model::validation::ValidatedResult;

use anyhow::anyhow;
use colored::Colorize;
use glob::glob;
use itertools::Itertools;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
enum ApplicationResolveMode {
    Automatic,
    Explicit(Vec<PathBuf>),
}

pub fn load(manifests: Vec<PathBuf>) -> anyhow::Result<Application> {
    let app_resolve_mode = if manifests.is_empty() {
        ApplicationResolveMode::Automatic
    } else {
        ApplicationResolveMode::Explicit(manifests)
    };
    to_anyhow(
        "Failed to load application manifest(s), see problems above".to_string(),
        load_validated(app_resolve_mode),
    )
}

fn to_anyhow<T>(message: String, result: ValidatedResult<T>) -> anyhow::Result<T> {
    fn print_warns(warns: Vec<String>) {
        let label = "Warning".yellow();
        for warn in warns {
            eprintln!("{}: {}", label, warn);
        }
    }

    fn print_errors(errors: Vec<String>) {
        let label = "Error".red();
        for error in errors {
            eprintln!("{}: {}", label, error);
        }
    }

    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(components, warns) => {
            print_warns(warns);
            println!();
            Ok(components)
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => {
            print_warns(warns);
            print_errors(errors);
            println!();

            Err(anyhow!(message))
        }
    }
}

fn load_validated(app_resolve_mode: ApplicationResolveMode) -> ValidatedResult<Application> {
    let sources = collect_sources(app_resolve_mode);
    let oam_apps = sources.and_then(|sources| {
        sources
            .into_iter()
            .map(|source| {
                ValidatedResult::from_result(oam::ApplicationWithSource::from_yaml_file(source))
                    .and_then(oam::ApplicationWithSource::validate)
            })
            .collect::<ValidatedResult<Vec<_>>>()
    });

    log_action("Collecting", "components");

    let app = oam_apps.and_then(Application::from_oam_apps);

    log_validated_action_result("Found", &app, |app| {
        if app.wasm_components_by_name.is_empty() {
            "no components".to_string()
        } else {
            format!(
                "components: {}",
                app.wasm_components_by_name.keys().join(", ")
            )
        }
    });

    app
}

fn collect_sources(mode: ApplicationResolveMode) -> ValidatedResult<Vec<PathBuf>> {
    log_action("Collecting", "sources");

    let sources = match mode {
        ApplicationResolveMode::Automatic => match find_main_source() {
            Some(source) => {
                std::env::set_current_dir(source.parent().expect("Failed ot get config parent"))
                    .expect("Failed to set current dir for config parent");

                match include_glob_patter_from_yaml_file(source.as_path()) {
                    Some(pattern) => ValidatedResult::from_result(
                        glob(pattern.as_str())
                            .map_err(|err| {
                                format!(
                                    "Failed to compile glob pattern: {}, source: {}, error: {}",
                                    pattern,
                                    source.to_string_lossy(),
                                    err
                                )
                            })
                            .and_then(|matches| {
                                matches.collect::<Result<Vec<_>, _>>().map_err(|err| {
                                    format!(
                                        "Failed to resolve glob pattern: {}, source: {}, error: {}",
                                        pattern,
                                        source.to_string_lossy(),
                                        err
                                    )
                                })
                            }),
                    )
                    .map(|mut sources| {
                        sources.insert(0, source);
                        sources
                    }),
                    None => ValidatedResult::Ok(vec![source]),
                }
            }
            None => ValidatedResult::from_error("No config file found!".to_string()),
        },
        ApplicationResolveMode::Explicit(sources) => {
            let non_unique_source_warns: Vec<_> = sources
                .iter()
                .counts()
                .into_iter()
                .filter(|(_, count)| *count > 1)
                .map(|(source, count)| {
                    format!(
                        "Source added multiple times, source: {}, count: {}",
                        source.to_string_lossy(),
                        count
                    )
                })
                .collect();

            ValidatedResult::from_value_and_warns(sources.clone(), non_unique_source_warns)
        }
    };

    log_validated_action_result("Found", &sources, |sources| {
        if sources.is_empty() {
            "no sources".to_string()
        } else {
            format!(
                "sources: {}",
                sources
                    .iter()
                    .map(|source| source.to_string_lossy())
                    .join(", ")
            )
        }
    });

    sources
}

fn find_main_source() -> Option<PathBuf> {
    let mut current_dir = std::env::current_dir().expect("Failed to get current dir");
    let mut last_source: Option<PathBuf> = None;

    loop {
        let file = current_dir.join(DEFAULT_CONFIG_FILE_NAME);
        if current_dir.join(DEFAULT_CONFIG_FILE_NAME).exists() {
            last_source = Some(file);
        }
        match current_dir.parent() {
            Some(parent_dir) => current_dir = parent_dir.to_path_buf(),
            None => {
                break;
            }
        }
    }

    last_source
}

// This a lenient non-validating peek for the include build property,
// as that is used early, during source collection
fn include_glob_patter_from_yaml_file(source: &Path) -> Option<String> {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| oam::Application::from_yaml_str(source.as_str()).ok())
        .and_then(|mut oam_app| {
            let mut includes = oam_app
                .spec
                .extract_components_by_type(&BTreeSet::from([OAM_COMPONENT_TYPE_WASM_BUILD]))
                .remove(OAM_COMPONENT_TYPE_WASM_BUILD)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|component| {
                    component
                        .typed_properties::<ComponentBuildProperties>()
                        .ok()
                        .and_then(|properties| properties.include)
                });

            match includes.next() {
                Some(include) => {
                    // Only return it if it's unique (if not it will cause validation errors later)
                    includes.next().is_none().then_some(include)
                }
                None => None,
            }
        })
}

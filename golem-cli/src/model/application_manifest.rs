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

use glob::glob;
use golem_common::model::{
    ComponentFilePath, ComponentFilePathWithPermissions, ComponentFilePermissions, ComponentType,
};
use golem_wasm_rpc_stubgen::commands::declarative::ApplicationResolveMode;
use golem_wasm_rpc_stubgen::model::oam;
use golem_wasm_rpc_stubgen::model::oam::TypedTraitProperties;
use golem_wasm_rpc_stubgen::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
use golem_wasm_rpc_stubgen::model::validation::{ValidatedResult, ValidationBuilder};
use golem_wasm_rpc_stubgen::model::wasm_rpc::{
    include_glob_patter_from_yaml_file, DEFAULT_CONFIG_FILE_NAME, OAM_COMPONENT_TYPE_WASM,
    OAM_COMPONENT_TYPE_WASM_BUILD, OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD, OAM_TRAIT_TYPE_WASM_RPC,
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tracing::warn;
use url::Url;

use super::GolemError;

// TODO: This is inlined from wasm-rpc. Refactor to avoid duplication.
pub fn load_app(
    app_resolve_mode: &ApplicationResolveMode,
) -> Result<ApplicationManifest, GolemError> {
    match load_app_validated(app_resolve_mode) {
        ValidatedResult::Ok(app) => Ok(app),
        ValidatedResult::OkWithWarns(app, warns) => {
            let warns = warns.join("\n");
            warn!("Encountered issues during app manifest loading: {}", warns);
            Ok(app)
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => {
            let warns = warns.join(",");
            let errors = errors.join(",");
            Err(GolemError(format!(
                "Failed loading app manifest. warns: {warns}, errors: {errors}"
            )))
        }
    }
}

fn load_app_validated(
    app_resolve_mode: &ApplicationResolveMode,
) -> ValidatedResult<ApplicationManifest> {
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

    oam_apps.and_then(ApplicationManifest::from_oam_apps)
}

fn collect_sources(mode: &ApplicationResolveMode) -> ValidatedResult<Vec<PathBuf>> {
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

// TODO: Taken from wasm-rpc. Refactor to avoid duplication.
#[derive(Clone, Debug)]
pub struct ApplicationManifest {
    pub common_wasm_build: Option<WasmBuild>,
    pub common_wasm_rpc_stub_build: Option<WasmRpcStubBuild>,
    pub wasm_rpc_stub_builds_by_name: BTreeMap<String, WasmRpcStubBuild>,
    pub wasm_components_by_name: BTreeMap<String, WasmComponent>,
}

impl ApplicationManifest {
    pub fn from_oam_apps(oam_apps: Vec<oam::ApplicationWithSource>) -> ValidatedResult<Self> {
        let mut validation = ValidationBuilder::new();

        let (all_components, all_wasm_builds, all_wasm_rpc_stub_builds) = {
            let mut all_components = Vec::<WasmComponent>::new();
            let mut all_wasm_builds = Vec::<WasmBuild>::new();
            let mut all_wasm_rpc_stub_builds = Vec::<WasmRpcStubBuild>::new();

            for mut oam_app in oam_apps {
                let (components, wasm_build, wasm_rpc_stub_build) =
                    Self::extract_and_convert_oam_components(&mut validation, &mut oam_app);
                all_components.extend(components);
                all_wasm_builds.extend(wasm_build);
                all_wasm_rpc_stub_builds.extend(wasm_rpc_stub_build);
            }

            (all_components, all_wasm_builds, all_wasm_rpc_stub_builds)
        };

        let wasm_components_by_name = Self::validate_components(&mut validation, all_components);

        let (common_wasm_rpc_stub_build, wasm_rpc_stub_builds_by_name) =
            Self::validate_wasm_rpc_stub_builds(
                &mut validation,
                &wasm_components_by_name,
                all_wasm_rpc_stub_builds,
            );

        let common_wasm_build = Self::validate_wasm_builds(&mut validation, all_wasm_builds);

        validation.build(Self {
            common_wasm_build,
            common_wasm_rpc_stub_build,
            wasm_rpc_stub_builds_by_name,
            wasm_components_by_name,
        })
    }

    fn extract_and_convert_oam_components(
        validation: &mut ValidationBuilder,
        oam_app: &mut oam::ApplicationWithSource,
    ) -> (Vec<WasmComponent>, Vec<WasmBuild>, Vec<WasmRpcStubBuild>) {
        validation.push_context("source", oam_app.source_as_string());

        let mut components_by_type =
            oam_app
                .application
                .spec
                .extract_components_by_type(&BTreeSet::from([
                    OAM_COMPONENT_TYPE_WASM,
                    OAM_COMPONENT_TYPE_WASM_BUILD,
                    OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD,
                ]));

        let wasm_components = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            OAM_COMPONENT_TYPE_WASM,
            Self::convert_wasm_component,
        );

        let wasm_builds = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            OAM_COMPONENT_TYPE_WASM_BUILD,
            Self::convert_wasm_build,
        );

        let wasm_rpc_stub_builds = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD,
            Self::convert_wasm_rpc_stub_build,
        );

        validation.add_warns(&oam_app.application.spec.components, |component| {
            Some((
                vec![
                    ("component name", component.name.clone()),
                    ("component type", component.component_type.clone()),
                ],
                "Unknown component-type".to_string(),
            ))
        });

        validation.pop_context();

        (wasm_components, wasm_builds, wasm_rpc_stub_builds)
    }

    fn convert_components<F, C>(
        source: &Path,
        validation: &mut ValidationBuilder,
        components_by_type: &mut BTreeMap<&'static str, Vec<oam::Component>>,
        component_type: &str,
        convert: F,
    ) -> Vec<C>
    where
        F: Fn(&Path, &mut ValidationBuilder, oam::Component) -> Option<C>,
    {
        components_by_type
            .remove(component_type)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|component| {
                validation.push_context("component name", component.name.clone());
                validation.push_context("component type", component.component_type.clone());
                let result = convert(source, validation, component);
                validation.pop_context();
                validation.pop_context();
                result
            })
            .collect()
    }

    fn convert_wasm_component(
        source: &Path,
        validation: &mut ValidationBuilder,
        mut component: oam::Component,
    ) -> Option<WasmComponent> {
        let properties = component
            .typed_properties::<WasmComponentProperties>()
            .map_err(|err| {
                validation.add_error(format!("Failed to get component properties: {}", err))
            })
            .ok();

        let wasm_rpc_traits = component
            .extract_traits_by_type(&BTreeSet::from([OAM_TRAIT_TYPE_WASM_RPC]))
            .remove(OAM_TRAIT_TYPE_WASM_RPC)
            .unwrap_or_default();

        let mut wasm_rpc_dependencies = Vec::<String>::new();

        for wasm_rpc in wasm_rpc_traits {
            validation.push_context("trait type", wasm_rpc.trait_type.clone());

            match WasmRpcTraitProperties::from_generic_trait(wasm_rpc) {
                Ok(wasm_rpc) => {
                    wasm_rpc.add_unknown_property_warns(
                        || vec![("dep component name", wasm_rpc.component_name.clone())],
                        validation,
                    );
                    wasm_rpc_dependencies.push(wasm_rpc.component_name)
                }
                Err(err) => validation
                    .add_error(format!("Failed to get wasm-rpc trait properties: {}", err)),
            }

            validation.pop_context();
        }

        let non_unique_wasm_rpc_dependencies = wasm_rpc_dependencies
            .iter()
            .counts()
            .into_iter()
            .filter(|(_, count)| *count > 1);

        validation.add_warns(
            non_unique_wasm_rpc_dependencies,
            |(dep_component_name, count)| {
                Some((
                    vec![],
                    format!(
                        "WASM RPC dependency specified multiple times for component: {}, count: {}",
                        dep_component_name, count
                    ),
                ))
            },
        );

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm component, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        let wasm_rpc_dependencies = wasm_rpc_dependencies
            .into_iter()
            .unique()
            .sorted()
            .collect::<Vec<_>>();

        let properties = properties?;

        properties.add_unknown_property_warns(Vec::new, validation);

        for build_step in &properties.build {
            let has_inputs = !build_step.inputs.is_empty();
            let has_outputs = !build_step.outputs.is_empty();

            if (has_inputs && !has_outputs) || (!has_inputs && has_outputs) {
                validation.push_context("command", build_step.command.clone());
                validation.add_warn(
                    "Using inputs and outputs only has effect when both defined".to_string(),
                );
                validation.pop_context();
            }
        }

        let converted_files = properties
            .files
            .into_iter()
            .flat_map(|file| Self::convert_component_file(validation, file, source))
            .collect();

        if validation.has_any_errors() {
            None
        } else {
            Some(WasmComponent {
                name: component.name,
                source: source.to_path_buf(),
                build_steps: properties.build,
                wit: properties.wit.into(),
                input_wasm: properties.input_wasm.into(),
                output_wasm: properties.output_wasm.into(),
                wasm_rpc_dependencies,
                component_type: properties
                    .component_type
                    .map_or_else(|| ComponentType::Durable, ComponentType::from),
                files: converted_files,
            })
        }
    }

    fn convert_wasm_build(
        source: &Path,
        validation: &mut ValidationBuilder,
        component: oam::Component,
    ) -> Option<WasmBuild> {
        let result = match component.typed_properties::<ComponentBuildProperties>() {
            Ok(properties) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                let wasm_rpc_stub_build = WasmBuild {
                    source: source.to_path_buf(),
                    name: component.name,
                    _build_dir: properties.build_dir.map(|s| s.into()),
                };

                Some(wasm_rpc_stub_build)
            }
            Err(err) => {
                validation.add_error(format!("Failed to get wasm build properties: {}", err));
                None
            }
        };

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm build, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        result
    }

    fn convert_wasm_rpc_stub_build(
        source: &Path,
        validation: &mut ValidationBuilder,
        component: oam::Component,
    ) -> Option<WasmRpcStubBuild> {
        let result = match component.typed_properties::<ComponentStubBuildProperties>() {
            Ok(properties) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                let wasm_rpc_stub_build = WasmRpcStubBuild {
                    source: source.to_path_buf(),
                    name: component.name,
                    component_name: properties.component_name,
                    build_dir: properties.build_dir.map(|s| s.into()),
                    wasm: properties.wasm.map(|s| s.into()),
                    wit: properties.wit.map(|s| s.into()),
                    _world: properties.world,
                    _always_inline_types: properties.always_inline_types,
                    _crate_version: properties.crate_version,
                    _wasm_rpc_path: properties.wasm_rpc_path,
                    _wasm_rpc_version: properties.wasm_rpc_version,
                };

                if wasm_rpc_stub_build.build_dir.is_some() && wasm_rpc_stub_build.wasm.is_some() {
                    validation.add_warn(
                        "Both buildDir and wasm fields are defined, wasm takes precedence"
                            .to_string(),
                    );
                }

                if wasm_rpc_stub_build.build_dir.is_some() && wasm_rpc_stub_build.wit.is_some() {
                    validation.add_warn(
                        "Both buildDir and wit fields are defined, wit takes precedence"
                            .to_string(),
                    );
                }

                if wasm_rpc_stub_build.component_name.is_some()
                    && wasm_rpc_stub_build.wasm.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) wasm rpc stub build the wasm field has no effect".to_string(),
                    );
                }

                if wasm_rpc_stub_build.component_name.is_some() && wasm_rpc_stub_build.wit.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) wasm rpc stub build the wit field has no effect".to_string(),
                    );
                }

                Some(wasm_rpc_stub_build)
            }
            Err(err) => {
                validation.add_error(format!(
                    "Failed to get wasm rpc stub build properties: {}",
                    err
                ));
                None
            }
        };

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm rpc stub build, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        result
    }

    fn convert_component_file(
        validation: &mut ValidationBuilder,
        file: RawComponentFile,
        source: &Path,
    ) -> Option<InitialComponentFile> {
        let source_path = DownloadableFile::make(&file.source_path, source)
            .map_err(|err| {
                validation.push_context("source path", file.source_path.to_string());
                validation.add_error(err);
                validation.pop_context();
            })
            .ok()?;

        Some(InitialComponentFile {
            source_path,
            target: ComponentFilePathWithPermissions {
                path: file.target_path,
                permissions: file
                    .permissions
                    .unwrap_or(ComponentFilePermissions::ReadOnly),
            },
        })
    }

    pub fn validate_components(
        validation: &mut ValidationBuilder,
        components: Vec<WasmComponent>,
    ) -> BTreeMap<String, WasmComponent> {
        let (wasm_components_by_name, sources) = {
            let mut wasm_components_by_name = BTreeMap::<String, WasmComponent>::new();
            let mut sources = BTreeMap::<String, Vec<String>>::new();
            for component in components {
                sources
                    .entry(component.name.clone())
                    .and_modify(|sources| sources.push(component.source_as_string()))
                    .or_insert_with(|| vec![component.source_as_string()]);
                wasm_components_by_name.insert(component.name.clone(), component);
            }
            (wasm_components_by_name, sources)
        };

        let non_unique_components = sources.into_iter().filter(|(_, sources)| sources.len() > 1);
        validation.add_errors(non_unique_components, |(component_name, sources)| {
            Some((
                vec![("component name", component_name)],
                format!(
                    "Component is specified multiple times in sources: {}",
                    sources.join(", ")
                ),
            ))
        });

        for (component_name, component) in &wasm_components_by_name {
            validation.push_context("source", component.source_as_string());

            validation.add_errors(&component.wasm_rpc_dependencies, |dep_component_name| {
                (!wasm_components_by_name.contains_key(dep_component_name)).then(|| {
                    (
                        vec![],
                        format!(
                            "Component {} references unknown component {} as dependency",
                            component_name, dep_component_name,
                        ),
                    )
                })
            });

            validation.pop_context();
        }

        wasm_components_by_name
    }

    fn validate_wasm_builds(
        validation: &mut ValidationBuilder,
        wasm_builds: Vec<WasmBuild>,
    ) -> Option<WasmBuild> {
        if wasm_builds.len() > 1 {
            validation.add_error(format!(
                "Component Build is specified multiple times in sources: {}",
                wasm_builds
                    .iter()
                    .map(|c| format!("{} in {}", c.name, c.source.to_string_lossy()))
                    .join(", ")
            ));
        }

        wasm_builds.into_iter().next()
    }

    fn validate_wasm_rpc_stub_builds(
        validation: &mut ValidationBuilder,
        wasm_components_by_name: &BTreeMap<String, WasmComponent>,
        wasm_rpc_stub_builds: Vec<WasmRpcStubBuild>,
    ) -> (Option<WasmRpcStubBuild>, BTreeMap<String, WasmRpcStubBuild>) {
        let (
            common_wasm_rpc_stub_builds,
            wasm_rpc_stub_builds_by_component_name,
            common_sources,
            sources,
        ) = {
            let mut common_wasm_rpc_stub_builds = Vec::<WasmRpcStubBuild>::new();
            let mut wasm_rpc_stub_builds_by_component_name =
                BTreeMap::<String, WasmRpcStubBuild>::new();

            let mut common_sources = Vec::<String>::new();
            let mut by_name_sources = BTreeMap::<String, Vec<String>>::new();

            for wasm_rpc_stub_build in wasm_rpc_stub_builds {
                match &wasm_rpc_stub_build.component_name {
                    Some(component_name) => {
                        by_name_sources
                            .entry(component_name.clone())
                            .and_modify(|sources| {
                                sources.push(wasm_rpc_stub_build.source_as_string())
                            })
                            .or_insert_with(|| vec![wasm_rpc_stub_build.source_as_string()]);
                        wasm_rpc_stub_builds_by_component_name
                            .insert(component_name.clone(), wasm_rpc_stub_build);
                    }
                    None => {
                        common_sources.push(wasm_rpc_stub_build.source_as_string());
                        common_wasm_rpc_stub_builds.push(wasm_rpc_stub_build)
                    }
                }
            }

            (
                common_wasm_rpc_stub_builds,
                wasm_rpc_stub_builds_by_component_name,
                common_sources,
                by_name_sources,
            )
        };

        let non_unique_wasm_rpc_stub_builds =
            sources.into_iter().filter(|(_, sources)| sources.len() > 1);

        validation.add_errors(
            non_unique_wasm_rpc_stub_builds,
            |(component_name, sources)| {
                Some((
                    vec![("component name", component_name)],
                    format!(
                        "Wasm rpc stub build is specified multiple times in sources: {}",
                        sources.join(", ")
                    ),
                ))
            },
        );

        if common_sources.len() > 1 {
            validation.add_error(
                format!(
                    "Common (without component name) wasm rpc build is specified multiple times in sources: {}",
                    common_sources.join(", "),
                )
            )
        }

        validation.add_errors(
            &wasm_rpc_stub_builds_by_component_name,
            |(component_name, wasm_rpc_stub_build)| {
                (!wasm_components_by_name.contains_key(component_name)).then(|| {
                    (
                        vec![("source", wasm_rpc_stub_build.source_as_string())],
                        format!(
                            "Wasm rpc stub build {} references unknown component {}",
                            wasm_rpc_stub_build.name, component_name
                        ),
                    )
                })
            },
        );

        (
            common_wasm_rpc_stub_builds.into_iter().next(),
            wasm_rpc_stub_builds_by_component_name,
        )
    }

    pub fn component(&self, component_name: &str) -> &WasmComponent {
        self.wasm_components_by_name
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_output_wasm(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.output_wasm.clone())
    }
}

#[derive(Clone, Debug)]
pub struct WasmComponent {
    pub name: String,
    pub source: PathBuf,
    pub build_steps: Vec<BuildStep>,
    pub wit: PathBuf,
    pub input_wasm: PathBuf,
    pub output_wasm: PathBuf,
    pub wasm_rpc_dependencies: Vec<String>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
}

impl WasmComponent {
    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }

    pub fn source_dir(&self) -> &Path {
        self.source
            .parent()
            .expect("Failed to get parent for source")
    }
}

#[derive(Clone, Debug)]
pub struct WasmBuild {
    source: PathBuf,
    name: String,
    _build_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct WasmRpcStubBuild {
    source: PathBuf,
    name: String,
    component_name: Option<String>,
    build_dir: Option<PathBuf>,
    wasm: Option<PathBuf>,
    wit: Option<PathBuf>,
    _world: Option<String>,
    _always_inline_types: Option<bool>,
    _crate_version: Option<String>,
    _wasm_rpc_path: Option<String>,
    _wasm_rpc_version: Option<String>,
}

impl WasmRpcStubBuild {
    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }

    pub fn source_dir(&self) -> &Path {
        self.source
            .parent()
            .expect("Failed to get parent for source")
    }
}

/// http, https, file, or protocol relative
#[derive(Clone, Debug)]
pub struct DownloadableFile(Url);

impl DownloadableFile {
    pub fn make(url_string: &str, relative_to: &Path) -> Result<Self, String> {
        // Try to parse the URL as an absolute URL
        let url = Url::parse(url_string).or_else(|_| {
            // If that fails, try to parse it as a relative path
            let canonical_relative_to = relative_to
                .parent()
                .expect("Failed to get parent")
                .canonicalize()
                .map_err(|_| {
                    format!(
                        "Failed to canonicalize relative path: {}",
                        relative_to.display()
                    )
                })?;

            let source_path = canonical_relative_to.join(PathBuf::from(url_string));
            Url::from_file_path(source_path)
                .map_err(|_| "Failed to convert path to URL".to_string())
        })?;

        let source_path_scheme = url.scheme();
        let supported_schemes = ["http", "https", "file", ""];
        if !supported_schemes.contains(&source_path_scheme) {
            return Err(format!(
                "Unsupported source path scheme: {source_path_scheme}"
            ));
        }
        Ok(DownloadableFile(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_url(self) -> Url {
        self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildStep {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct InitialComponentFile {
    pub source_path: DownloadableFile,
    pub target: ComponentFilePathWithPermissions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawComponentType {
    Ephemeral,
    Durable,
}

impl From<RawComponentType> for ComponentType {
    fn from(raw: RawComponentType) -> Self {
        match raw {
            RawComponentType::Ephemeral => Self::Ephemeral,
            RawComponentType::Durable => Self::Durable,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RawComponentFile {
    /// http, https, file, or protocol relative
    #[serde(rename = "sourcePath")]
    pub source_path: String,

    #[serde(rename = "targetPath")]
    pub target_path: ComponentFilePath,
    pub permissions: Option<ComponentFilePermissions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WasmComponentProperties {
    pub wit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildStep>,
    #[serde(rename = "inputWasm")]
    pub input_wasm: String,
    #[serde(rename = "outputWasm")]
    pub output_wasm: String,
    #[serde(rename = "componentType")]
    pub component_type: Option<RawComponentType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<RawComponentFile>,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl HasUnknownProperties for WasmComponentProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

impl oam::TypedComponentProperties for WasmComponentProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WasmRpcTraitProperties {
    #[serde(rename = "componentName")]
    pub component_name: String,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl HasUnknownProperties for WasmRpcTraitProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

impl oam::TypedTraitProperties for WasmRpcTraitProperties {
    fn trait_type() -> &'static str {
        OAM_TRAIT_TYPE_WASM_RPC
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComponentBuildProperties {
    include: Option<String>,
    build_dir: Option<String>,
    #[serde(flatten)]
    unknown_properties: UnknownProperties,
}

impl oam::TypedComponentProperties for ComponentBuildProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM_BUILD
    }
}

impl HasUnknownProperties for ComponentBuildProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComponentStubBuildProperties {
    component_name: Option<String>,
    build_dir: Option<String>,
    wasm: Option<String>,
    wit: Option<String>,
    world: Option<String>,
    always_inline_types: Option<bool>,
    crate_version: Option<String>,
    wasm_rpc_path: Option<String>,
    wasm_rpc_version: Option<String>,
    #[serde(flatten)]
    unknown_properties: UnknownProperties,
}

impl oam::TypedComponentProperties for ComponentStubBuildProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD
    }
}

impl HasUnknownProperties for ComponentStubBuildProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

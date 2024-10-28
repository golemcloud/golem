use crate::model::oam;
use crate::model::oam::TypedTraitProperties;
use crate::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
use crate::model::validation::{ValidatedResult, ValidationBuilder};
use crate::naming;
use golem_wasm_rpc::WASM_RPC_VERSION;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

pub const OAM_TRAIT_TYPE_WASM_RPC: &str = "wasm-rpc";

pub const OAM_COMPONENT_TYPE_WASM: &str = "wasm";
pub const OAM_COMPONENT_TYPE_WASM_BUILD: &str = "wasm-build";
pub const OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD: &str = "wasm-rpc-stub-build";

pub fn init_oam_app(_component_name: String) -> oam::Application {
    // TODO: let's do it as part of https://github.com/golemcloud/wasm-rpc/issues/89
    todo!()
}

// This a lenient non-validating peek for the include build property,
// as that is used early, during source collection
pub fn include_glob_patter_from_yaml_file(source: &Path) -> Option<String> {
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

#[derive(Clone, Debug)]
pub struct Application {
    pub common_wasm_build: Option<WasmBuild>,
    pub common_wasm_rpc_stub_build: Option<WasmRpcStubBuild>,
    pub wasm_rpc_stub_builds_by_name: BTreeMap<String, WasmRpcStubBuild>,
    pub wasm_components_by_name: BTreeMap<String, WasmComponent>,
}

impl Application {
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
        let properties = component.typed_properties::<WasmComponentProperties>();

        if let Some(err) = properties.as_ref().err() {
            validation.add_error(format!("Failed to get component properties: {}", err))
        }

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

        match (properties, validation.has_any_errors()) {
            (Ok(properties), false) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                for build_step in &properties.build {
                    let has_inputs = !build_step.inputs.is_empty();
                    let has_outputs = !build_step.outputs.is_empty();

                    if (has_inputs && !has_outputs) || (!has_inputs && has_outputs) {
                        validation.push_context("command", build_step.command.clone());
                        validation.add_warn(
                            "Using inputs and outputs only has effect when both defined"
                                .to_string(),
                        );
                        validation.pop_context();
                    }
                }

                Some(WasmComponent {
                    name: component.name,
                    source: source.to_path_buf(),
                    build_steps: properties.build,
                    wit: properties.wit.into(),
                    input_wasm: properties.input_wasm.into(),
                    output_wasm: properties.output_wasm.into(),
                    wasm_rpc_dependencies,
                })
            }
            _ => None,
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
                    build_dir: properties.build_dir.map(|s| s.into()),
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
                    world: properties.world,
                    always_inline_types: properties.always_inline_types,
                    crate_version: properties.crate_version,
                    wasm_rpc_path: properties.wasm_rpc_path,
                    wasm_rpc_version: properties.wasm_rpc_version,
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
                    .map(|c| format!("{} in {}", c.name, c.source.display()))
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

    pub fn all_wasm_rpc_dependencies(&self) -> BTreeSet<String> {
        self.wasm_components_by_name
            .iter()
            .flat_map(|(_, component)| {
                component
                    .wasm_rpc_dependencies
                    .iter()
                    .map(|component_name| component_name.to_string())
            })
            .collect()
    }

    pub fn build_dir(&self) -> PathBuf {
        self.common_wasm_build
            .as_ref()
            .and_then(|build| build.build_dir.clone())
            .unwrap_or_else(|| PathBuf::from("build"))
    }

    pub fn component(&self, component_name: &str) -> &WasmComponent {
        self.wasm_components_by_name
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_wit(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.wit.clone())
    }

    pub fn component_input_wasm(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.input_wasm.clone())
    }

    pub fn component_output_wasm(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.output_wasm.clone())
    }

    pub fn stub_world(&self, component_name: &str) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.world.clone())
            .flatten()
    }

    pub fn stub_crate_version(&self, component_name: &str) -> String {
        self.stub_gen_property(component_name, |build| build.crate_version.clone())
            .flatten()
            .unwrap_or_else(|| WASM_RPC_VERSION.to_string())
    }

    pub fn stub_always_inline_types(&self, component_name: &str) -> bool {
        self.stub_gen_property(component_name, |build| build.always_inline_types)
            .flatten()
            .unwrap_or(false)
    }

    pub fn stub_wasm_rpc_path(&self, component_name: &str) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.wasm_rpc_path.clone())
            .flatten()
    }

    pub fn stub_wasm_rpc_version(&self, component_name: &str) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.wasm_rpc_version.clone())
            .flatten()
    }

    pub fn stub_build_dir(&self, component_name: &str) -> PathBuf {
        self.stub_gen_property(component_name, |build| {
            build
                .build_dir
                .as_ref()
                .map(|build_dir| build.source_dir().join(build_dir))
        })
        .flatten()
        .unwrap_or_else(|| self.build_dir())
        .join("stub")
    }

    pub fn stub_wasm(&self, component_name: &str) -> PathBuf {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .and_then(|build| {
                build
                    .wasm
                    .as_ref()
                    .map(|wasm| build.source_dir().join(wasm))
            })
            .unwrap_or_else(|| {
                self.stub_build_dir(component_name)
                    .join(component_name)
                    .join("stub.wasm")
            })
    }

    pub fn stub_wit(&self, component_name: &str) -> PathBuf {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .and_then(|build| build.wit.as_ref().map(|wit| build.source_dir().join(wit)))
            .unwrap_or_else(|| {
                self.stub_build_dir(component_name)
                    .join(component_name)
                    .join(naming::wit::WIT_DIR)
            })
    }

    fn stub_gen_property<T, F>(&self, component_name: &str, get_property: F) -> Option<T>
    where
        F: Fn(&WasmRpcStubBuild) -> T,
    {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .map(&get_property)
            .or_else(|| self.common_wasm_rpc_stub_build.as_ref().map(get_property))
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WasmComponentProperties {
    pub wit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildStep>,
    #[serde(rename = "inputWasm")]
    pub input_wasm: String,
    #[serde(rename = "outputWasm")]
    pub output_wasm: String,
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
pub struct WasmRpcTraitProperties {
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
pub struct ComponentBuildProperties {
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

#[derive(Clone, Debug)]
pub struct WasmBuild {
    source: PathBuf,
    name: String,
    build_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStubBuildProperties {
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

#[derive(Clone, Debug)]
pub struct WasmRpcStubBuild {
    source: PathBuf,
    name: String,
    component_name: Option<String>,
    build_dir: Option<PathBuf>,
    wasm: Option<PathBuf>,
    wit: Option<PathBuf>,
    world: Option<String>,
    always_inline_types: Option<bool>,
    crate_version: Option<String>,
    wasm_rpc_path: Option<String>,
    wasm_rpc_version: Option<String>,
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::model::oam;
    use assert2::assert;

    #[test]
    fn load_app_with_warns() {
        let oam_app: oam::ApplicationWithSource = oam_app_one();
        let (app, warns, errors) = Application::from_oam_apps(vec![oam_app]).into_product();

        assert!(app.is_some());
        let app = app.unwrap();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(app.wasm_components_by_name.len() == 3);
        assert!(warns.len() == 3);
        assert!(errors.len() == 0);

        let (component_name, component) = app.wasm_components_by_name.iter().next().unwrap();

        assert!(component_name == "component-one");
        assert!(component.name == "component-one");
        assert!(component.wit.to_string_lossy() == "wit");
        assert!(component.input_wasm.to_string_lossy() == "out/in.wasm");
        assert!(component.output_wasm.to_string_lossy() == "out/out.wasm");
        assert!(component.wasm_rpc_dependencies.len() == 2);

        assert!(component.wasm_rpc_dependencies[0] == "component-three");
        assert!(component.wasm_rpc_dependencies[1] == "component-two");
    }

    #[test]
    fn load_app_with_warns_and_errors() {
        let oam_app: oam::ApplicationWithSource = oam_app_two();
        let (_app, warns, errors) = Application::from_oam_apps(vec![oam_app]).into_product();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(errors.len() == 2);

        assert!(errors[0].contains("component-one"));
        assert!(errors[0].contains("component-three"));
        assert!(errors[0].contains("test-oam-app-two.yaml"));

        assert!(errors[1].contains("component-one"));
        assert!(errors[1].contains("component-two"));
        assert!(errors[1].contains("test-oam-app-two.yaml"));
    }

    fn oam_app_one() -> oam::ApplicationWithSource {
        oam::ApplicationWithSource::from_yaml_string(
            "test-oam-app-one.yaml".into(),
            r#"
apiVersion: core.oam.dev/v1beta1
metadata:
  name: "App name"
kind: Application
spec:
  components:
    - name: component-one
      type: wasm
      properties:
        wit: wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
        testUnknownProp: test
      traits:
        - type: wasm-rpc
          properties:
            componentName: component-two
        - type: wasm-rpc
          properties:
            componentName: component-three
            testUnknownProp: test
        - type: unknown-trait
          properties:
            testUnknownProp: test
    - name: component-two
      type: wasm
      properties:
        wit: wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
    - name: component-three
      type: wasm
      properties:
        wit: wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
"#
            .to_string(),
        )
        .unwrap()
    }

    fn oam_app_two() -> oam::ApplicationWithSource {
        oam::ApplicationWithSource::from_yaml_string(
            "test-oam-app-two.yaml".into(),
            r#"
apiVersion: core.oam.dev/v1beta1
metadata:
  name: "App name"
kind: Application
spec:
  components:
    - name: component-one
      type: wasm
      properties:
        wit: wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
        testUnknownProp: test
      traits:
        - type: wasm-rpc
          properties:
            componentName: component-two
        - type: wasm-rpc
          properties:
            componentName: component-three
            testUnknownProp: test
        - type: unknown-trait
          properties:
            testUnknownProp: test
    - name: component-one
      type: unknown-component-type
      properties:
"#
            .to_string(),
        )
        .unwrap()
    }
}

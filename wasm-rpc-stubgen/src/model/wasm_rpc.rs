use crate::model::oam;
use crate::model::oam::TypedTraitProperties;
use crate::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
use crate::model::validation::{ValidatedResult, ValidationBuilder};
use golem_wasm_rpc::WASM_RPC_VERSION;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

pub const OAM_TRAIT_TYPE_WASM_RPC: &str = "wasm-rpc";

pub const OAM_COMPONENT_TYPE_DURABLE: &str = "durable";
pub const OAM_COMPONENT_TYPE_EPHEMERAL: &str = "ephemeral";
pub const OAM_COMPONENT_TYPE_COMPONENT_STUB_BUILD: &str = "component-stub-build";

// TODO: let's create samples directly in yaml / yaml templates with comments
pub fn init_oam_app(component_name: String) -> oam::Application {
    let component = {
        let mut component = oam::Component {
            name: component_name.clone(),
            component_type: OAM_COMPONENT_TYPE_DURABLE.to_string(),
            properties: Default::default(),
            traits: vec![],
        };

        component.set_typed_properties(DurableComponentProperties {
            common: CommonComponentProperties {
                wit: "wit".to_string(),
                input_wasm: "target/input.wasm".to_string(),
                output_wasm: "target/output.wasm".to_string(),
                unknown_properties: Default::default(),
            },
        });

        component.add_typed_trait(WasmRpcTraitProperties {
            component_name: component_name.clone(),
            unknown_properties: Default::default(),
        });

        component
    };

    let mut app = oam::Application::new(component_name);
    app.spec.components.push(component);

    app
}

#[derive(Clone, Debug)]
pub struct Application {
    pub common_component_stub_build: Option<ComponentStubBuild>,
    pub component_stub_builds_by_name: BTreeMap<String, ComponentStubBuild>,
    pub components_by_name: BTreeMap<String, Component>,
}

impl Application {
    pub fn from_oam_apps(oam_apps: Vec<oam::ApplicationWithSource>) -> ValidatedResult<Self> {
        let mut validation = ValidationBuilder::new();

        let (all_components, all_component_stub_builds) = {
            let mut all_components = Vec::<Component>::new();
            let mut all_component_stub_builds = Vec::<ComponentStubBuild>::new();

            for mut oam_app in oam_apps {
                let (components, component_stub_build) =
                    Self::extract_and_convert_oam_components(&mut validation, &mut oam_app);
                all_components.extend(components);
                all_component_stub_builds.extend(component_stub_build);
            }

            (all_components, all_component_stub_builds)
        };

        let components_by_name = Self::validate_components(&mut validation, all_components);

        let (common_component_stub_build, component_stub_builds_by_name) =
            Self::validate_component_stub_builds(
                &mut validation,
                &components_by_name,
                all_component_stub_builds,
            );

        validation.build(Self {
            common_component_stub_build,
            component_stub_builds_by_name,
            components_by_name,
        })
    }

    fn extract_and_convert_oam_components(
        validation: &mut ValidationBuilder,
        oam_app: &mut oam::ApplicationWithSource,
    ) -> (Vec<Component>, Vec<ComponentStubBuild>) {
        validation.push_context("source", oam_app.source_as_string());

        // Extract components and partition by type
        let mut components_by_type =
            oam_app
                .application
                .spec
                .extract_components_by_type(&BTreeSet::from([
                    OAM_COMPONENT_TYPE_DURABLE,
                    OAM_COMPONENT_TYPE_EPHEMERAL,
                    OAM_COMPONENT_TYPE_COMPONENT_STUB_BUILD,
                ]));

        // Convert durable and ephemeral components
        let components = {
            let mut get_oam_components_with_type = |type_str: &str, type_enum: ComponentType| {
                components_by_type
                    .remove(type_str)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |c| (type_enum.clone(), c))
            };

            let durable_oam_components =
                get_oam_components_with_type(OAM_COMPONENT_TYPE_DURABLE, ComponentType::Durable);
            let ephemeral_oam_components = get_oam_components_with_type(
                OAM_COMPONENT_TYPE_EPHEMERAL,
                ComponentType::Ephemeral,
            );
            let all_oam_components = durable_oam_components.chain(ephemeral_oam_components);

            all_oam_components
                .into_iter()
                .filter_map(|(component_type, component)| {
                    Self::convert_component(&oam_app.source, validation, component_type, component)
                })
                .collect::<Vec<_>>()
        };

        // Convert stub builds
        let component_stub_builds = {
            let oam_components = components_by_type
                .remove(OAM_COMPONENT_TYPE_COMPONENT_STUB_BUILD)
                .unwrap_or_default();

            let mut components = Vec::<ComponentStubBuild>::new();
            components.reserve(oam_components.len());

            oam_components
                .into_iter()
                .filter_map(|component| {
                    Self::convert_component_stub_build(&oam_app.source, validation, component)
                })
                .collect::<Vec<_>>()
        };

        // Warn on unknown components
        validation.add_warns(&oam_app.application.spec.components, |component| {
            Some((
                vec![
                    ("component name", component.name.clone()),
                    ("component type", component.component_type.clone()),
                ],
                "Unknown component-type".to_string(),
            ))
        });

        (components, component_stub_builds)
    }

    fn convert_component(
        source: &Path,
        validation: &mut ValidationBuilder,
        component_type: ComponentType,
        mut component: oam::Component,
    ) -> Option<Component> {
        validation.push_context("component name", component.name.clone());
        validation.push_context("component type", component.component_type.clone());

        let properties = match component.component_type.as_str() {
            OAM_COMPONENT_TYPE_DURABLE => component
                .get_typed_properties::<DurableComponentProperties>()
                .map(|p| p.common),
            OAM_COMPONENT_TYPE_EPHEMERAL => component
                .get_typed_properties::<EphemeralComponentProperties>()
                .map(|p| p.common),
            other => panic!("Unexpected component type: {}", other),
        };

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

        let wasm_rpc_dependencies = wasm_rpc_dependencies
            .into_iter()
            .unique()
            .sorted()
            .collect::<Vec<_>>();

        let component = match (properties, validation.has_any_errors()) {
            (Ok(properties), false) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                Some(Component {
                    name: component.name,
                    component_type,
                    source: source.to_path_buf(),
                    wit: properties.wit.into(),
                    input_wasm: properties.input_wasm.into(),
                    output_wasm: properties.output_wasm.into(),
                    wasm_rpc_dependencies,
                })
            }
            _ => None,
        };

        validation.pop_context();
        validation.pop_context();

        component
    }

    fn convert_component_stub_build(
        source: &Path,
        validation: &mut ValidationBuilder,
        component: oam::Component,
    ) -> Option<ComponentStubBuild> {
        validation.push_context("component stub build name", component.name.clone());

        let result = match component.get_typed_properties::<ComponentStubBuildProperties>() {
            Ok(properties) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                let component_stub_build = ComponentStubBuild {
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

                if component_stub_build.build_dir.is_some() && component_stub_build.wasm.is_some() {
                    validation.add_warn(
                        "Both buildDir and wasm fields are defined, wasm takes precedence"
                            .to_string(),
                    );
                }

                if component_stub_build.build_dir.is_some() && component_stub_build.wit.is_some() {
                    validation.add_warn(
                        "Both buildDir and wit fields are defined, wit takes precedence"
                            .to_string(),
                    );
                }

                if component_stub_build.component_name.is_some()
                    && component_stub_build.wasm.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) component stub builds the wasm field has no effect".to_string(),
                    );
                }

                if component_stub_build.component_name.is_some()
                    && component_stub_build.wit.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) component stub builds the wit field has no effect".to_string(),
                    );
                }

                Some(component_stub_build)
            }
            Err(err) => {
                validation.add_error(format!(
                    "Failed to get component stub build properties: {}",
                    err
                ));
                None
            }
        };

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for component stub build, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        validation.pop_context();

        result
    }

    pub fn validate_components(
        validation: &mut ValidationBuilder,
        components: Vec<Component>,
    ) -> BTreeMap<String, Component> {
        let (components_by_name, sources) = {
            let mut components_by_name = BTreeMap::<String, Component>::new();
            let mut sources = BTreeMap::<String, Vec<String>>::new();
            for component in components {
                sources
                    .entry(component.name.clone())
                    .and_modify(|sources| sources.push(component.source_as_string()))
                    .or_insert_with(|| vec![component.source_as_string()]);
                components_by_name.insert(component.name.clone(), component);
            }
            (components_by_name, sources)
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

        for (component_name, component) in &components_by_name {
            validation.push_context("source", component.source_as_string());

            validation.add_errors(&component.wasm_rpc_dependencies, |dep_component_name| {
                (!components_by_name.contains_key(dep_component_name)).then(|| {
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

        components_by_name
    }

    fn validate_component_stub_builds(
        validation: &mut ValidationBuilder,
        components_by_name: &BTreeMap<String, Component>,
        component_stub_builds: Vec<ComponentStubBuild>,
    ) -> (
        Option<ComponentStubBuild>,
        BTreeMap<String, ComponentStubBuild>,
    ) {
        let (
            common_component_stub_builds,
            component_stub_builds_by_component_name,
            common_sources,
            sources,
        ) = {
            let mut common_component_stub_builds = Vec::<ComponentStubBuild>::new();
            let mut component_stub_builds_by_component_name =
                BTreeMap::<String, ComponentStubBuild>::new();

            let mut common_sources = Vec::<String>::new();
            let mut by_name_sources = BTreeMap::<String, Vec<String>>::new();

            for component_stub_build in component_stub_builds {
                match &component_stub_build.component_name {
                    Some(component_name) => {
                        by_name_sources
                            .entry(component_name.clone())
                            .and_modify(|sources| {
                                sources.push(component_stub_build.source_as_string())
                            })
                            .or_insert_with(|| vec![component_stub_build.source_as_string()]);
                        component_stub_builds_by_component_name
                            .insert(component_name.clone(), component_stub_build);
                    }
                    None => {
                        common_sources.push(component_stub_build.source_as_string());
                        common_component_stub_builds.push(component_stub_build)
                    }
                }
            }

            (
                common_component_stub_builds,
                component_stub_builds_by_component_name,
                common_sources,
                by_name_sources,
            )
        };

        let non_unique_component_stub_builds =
            sources.into_iter().filter(|(_, sources)| sources.len() > 1);

        validation.add_errors(
            non_unique_component_stub_builds,
            |(component_name, sources)| {
                Some((
                    vec![("component name", component_name)],
                    format!(
                        "Component Stub Build is specified multiple times in sources: {}",
                        sources.join(", ")
                    ),
                ))
            },
        );

        if common_sources.len() > 1 {
            validation.add_error(
                format!(
                    "Common (without component name) Component Stub Build is specified multiple times in sources: {}",
                    common_sources.join(", "),
                )
            )
        }

        validation.add_errors(
            &component_stub_builds_by_component_name,
            |(component_name, component_stub_build)| {
                (!components_by_name.contains_key(component_name)).then(|| {
                    (
                        vec![("source", component_stub_build.source_as_string())],
                        format!(
                            "Component Stub Build {} references unknown component {}",
                            component_stub_build.name, component_name
                        ),
                    )
                })
            },
        );

        (
            common_component_stub_builds.into_iter().next(),
            component_stub_builds_by_component_name,
        )
    }

    pub fn all_wasm_rpc_dependencies(&self) -> BTreeSet<String> {
        self.components_by_name
            .iter()
            .flat_map(|(_, component)| {
                component
                    .wasm_rpc_dependencies
                    .iter()
                    .map(|component_name| component_name.to_string())
            })
            .collect()
    }

    pub fn component(&self, component_name: &str) -> &Component {
        self.components_by_name
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_wit(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.wit.clone())
    }

    pub fn stub_source_wit_root(&self, component_name: &str) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(component.wit.clone())
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
        .unwrap_or_else(|| PathBuf::from("build"))
    }

    pub fn stub_dest_wasm(&self, component_name: &str) -> PathBuf {
        self.component_stub_builds_by_name
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

    pub fn stub_dest_wit(&self, component_name: &str) -> PathBuf {
        self.component_stub_builds_by_name
            .get(component_name)
            .and_then(|build| build.wit.as_ref().map(|wit| build.source_dir().join(wit)))
            .unwrap_or_else(|| {
                self.stub_build_dir(component_name)
                    .join(component_name)
                    .join("wit")
            })
    }

    fn stub_gen_property<T, F>(&self, component_name: &str, get_property: F) -> Option<T>
    where
        F: Fn(&ComponentStubBuild) -> T,
    {
        self.component_stub_builds_by_name
            .get(component_name)
            .map(&get_property)
            .or_else(|| self.common_component_stub_build.as_ref().map(get_property))
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    pub component_type: ComponentType,
    pub source: PathBuf,
    pub wit: PathBuf,
    pub input_wasm: PathBuf,
    pub output_wasm: PathBuf,
    pub wasm_rpc_dependencies: Vec<String>,
}

impl Component {
    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }

    pub fn source_dir(&self) -> &Path {
        self.source
            .parent()
            .expect("Failed to get parent for source")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentType {
    Durable,
    Ephemeral,
}

impl TryFrom<&str> for ComponentType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            OAM_COMPONENT_TYPE_DURABLE => Ok(ComponentType::Durable),
            OAM_COMPONENT_TYPE_EPHEMERAL => Ok(ComponentType::Ephemeral),
            other => Err(format!("Unknown component type: {}", other)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommonComponentProperties {
    pub wit: String,
    #[serde(rename = "inputWasm")]
    pub input_wasm: String,
    #[serde(rename = "outputWasm")]
    pub output_wasm: String,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl HasUnknownProperties for CommonComponentProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableComponentProperties {
    #[serde(flatten)]
    pub common: CommonComponentProperties,
}

impl oam::TypedComponentProperties for DurableComponentProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_DURABLE
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EphemeralComponentProperties {
    #[serde(flatten)]
    pub common: CommonComponentProperties,
}

impl oam::TypedComponentProperties for EphemeralComponentProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_EPHEMERAL
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
        OAM_COMPONENT_TYPE_COMPONENT_STUB_BUILD
    }
}

impl HasUnknownProperties for ComponentStubBuildProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

#[derive(Clone, Debug)]
pub struct ComponentStubBuild {
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

impl ComponentStubBuild {
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
    use super::*;
    use crate::model::oam;
    use assert2::assert;

    #[test]
    fn oam_app_to_components() {
        let oam_app: oam::ApplicationWithSource = oam_app_one();
        assert!(oam_app.application.api_version == oam::API_VERSION_V1BETA1);
        assert!(oam_app.application.kind == oam::KIND_APPLICATION);
        assert!(oam_app.application.metadata.name == "App name");
        assert!(oam_app.application.spec.components.len() == 2);

        let (components, warns, errors) = Component::from_oam_application(oam_app).into_product();

        assert!(components.is_some());
        let components = components.unwrap();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(components.len() == 1);
        assert!(warns.len() == 3);
        assert!(errors.len() == 0);

        let component = &components[0];

        assert!(component.name == "component-one");
        assert!(component.component_type == ComponentType::Durable);
        assert!(component.wit.to_string_lossy() == "wit");
        assert!(component.input_wasm.to_string_lossy() == "out/in.wasm");
        assert!(component.output_wasm.to_string_lossy() == "out/out.wasm");
        assert!(component.wasm_rpc_dependencies.len() == 2);

        assert!(component.wasm_rpc_dependencies[0] == "component-two");
        assert!(component.wasm_rpc_dependencies[1] == "component-three");
    }

    #[test]
    fn oam_app_to_wasm_rpc_app_with_missing_deps() {
        let application =
            Component::from_oam_application(oam_app_one()).and_then(Application::from_components);

        let (_app, warns, errors) = application.into_product();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(errors.len() == 2);

        assert!(errors[0].contains("component-one"));
        assert!(errors[0].contains("component-two"));
        assert!(errors[0].contains("test-oam-app-one.yaml"));

        assert!(errors[0].contains("component-one"));
        assert!(errors[1].contains("component-three"));
        assert!(errors[1].contains("test-oam-app-one.yaml"));
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
      type: durable
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

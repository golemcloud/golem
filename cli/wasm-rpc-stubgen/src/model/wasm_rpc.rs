use crate::model::oam;
use crate::model::oam::TypedTraitProperties;
use crate::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
use crate::model::validation::{ValidatedResult, ValidationBuilder};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

pub const TRAIT_TYPE_WASM_RPC: &str = "wasm-rpc";

pub const COMPONENT_TYPE_DURABLE: &str = "durable";
pub const COMPONENT_TYPE_EPHEMERAL: &str = "ephemeral";

pub fn init_oam_app(component_name: String) -> oam::Application {
    let component = {
        let mut component = oam::Component {
            name: component_name.clone(),
            component_type: COMPONENT_TYPE_DURABLE.to_string(),
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
    pub metadata: oam::Metadata,
    pub components_by_name: BTreeMap<String, Component>,
}

impl Application {
    pub fn from_components(components: Vec<Component>) -> ValidatedResult<Self> {
        let mut component_sources = BTreeMap::<String, Vec<String>>::new();

        let components_by_name = {
            let mut components_by_name = BTreeMap::<String, Component>::new();
            for component in components {
                component_sources
                    .entry(component.name.clone())
                    .and_modify(|sources| sources.push(component.source_as_string()))
                    .or_insert_with(|| vec![component.source_as_string()]);
                components_by_name.insert(component.name.clone(), component);
            }
            components_by_name
        };

        let mut validation = ValidationBuilder::new();

        let non_unique_components = component_sources
            .into_iter()
            .filter(|(_, sources)| sources.len() > 1);

        for (component_name, sources) in non_unique_components {
            validation.push_context("component name", component_name);
            validation.add_error(format!(
                "Component is specified multiple times in sources: {}",
                sources.join(", ")
            ));
            validation.pop_context();
        }

        for (component_name, component) in &components_by_name {
            validation.push_context("source", component.source_as_string());

            for dep_component_name in &component.wasm_rpc_dependencies {
                if !components_by_name.contains_key(dep_component_name) {
                    validation.add_error(format!(
                        "Component {} references unknown component {} as dependency",
                        component_name, dep_component_name,
                    ));
                }
            }

            validation.pop_context();
        }

        validation.build(Self { components_by_name })
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

    pub fn resolve_component_path(&self) -> &Path {
        self.source.parent().unwrap_or_else(|| {
            panic!(
                "Failed to get component path for source: {}",
                self.source.to_string_lossy()
            )
        })
    }

    pub fn resolve_wit_path(&self) -> PathBuf {
        self.resolve_component_path().join(self.wit.as_path())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentType {
    Durable,
    Ephemeral,
}

impl TryFrom<&str> for ComponentType {
    type Error = String;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            COMPONENT_TYPE_DURABLE => Ok(ComponentType::Durable),
            COMPONENT_TYPE_EPHEMERAL => Ok(ComponentType::Ephemeral),
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
        COMPONENT_TYPE_DURABLE
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EphemeralComponentProperties {
    #[serde(flatten)]
    pub common: CommonComponentProperties,
}

impl oam::TypedComponentProperties for EphemeralComponentProperties {
    fn component_type() -> &'static str {
        COMPONENT_TYPE_EPHEMERAL
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
        TRAIT_TYPE_WASM_RPC
    }
}

impl Component {
    pub fn from_oam_application(
        mut application: oam::ApplicationWithSource,
    ) -> ValidatedResult<Vec<Component>> {
        let mut validation = ValidationBuilder::new();
        validation.push_context("source", application.source_as_string());

        let mut components = Vec::<Component>::new();

        if application.application.spec.components.is_empty() {
            validation.add_error("Expected at least one component specification".to_string());
        } else {
            let mut components_by_type =
                application
                    .application
                    .spec
                    .extract_components_by_type(&BTreeSet::from([
                        COMPONENT_TYPE_DURABLE,
                        COMPONENT_TYPE_EPHEMERAL,
                    ]));

            let mut get_components_with_type = |type_str: &str, type_enum: ComponentType| {
                components_by_type
                    .remove(type_str)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |c| (type_enum.clone(), c))
            };

            let durable_components =
                get_components_with_type(COMPONENT_TYPE_DURABLE, ComponentType::Durable);
            let ephemeral_components =
                get_components_with_type(COMPONENT_TYPE_EPHEMERAL, ComponentType::Ephemeral);

            let all_components = durable_components.chain(ephemeral_components);

            for (component_type, mut component) in all_components {
                validation.push_context("component name", component.name.clone());
                validation.push_context("component type", component.component_type.clone());

                let properties = match component.component_type.as_str() {
                    COMPONENT_TYPE_DURABLE => component
                        .get_typed_properties::<DurableComponentProperties>()
                        .map(|p| p.common),
                    COMPONENT_TYPE_EPHEMERAL => component
                        .get_typed_properties::<EphemeralComponentProperties>()
                        .map(|p| p.common),
                    other => panic!("Unexpected component type: {}", other),
                };

                if let Some(err) = properties.as_ref().err() {
                    validation.add_error(format!("Failed to get component properties: {}", err))
                }

                let wasm_rpc_traits = component
                    .extract_traits_by_type(&BTreeSet::from([TRAIT_TYPE_WASM_RPC]))
                    .remove(TRAIT_TYPE_WASM_RPC)
                    .unwrap_or_default();

                let mut wasm_rpc_dependencies = Vec::<String>::new();
                for wasm_rpc in wasm_rpc_traits {
                    validation.push_context("trait type", wasm_rpc.trait_type.clone());

                    match WasmRpcTraitProperties::from_generic_trait(wasm_rpc) {
                        Ok(wasm_rpc) => {
                            wasm_rpc.add_unknown_property_warns(
                                || vec![("dep component name", wasm_rpc.component_name.clone())],
                                &mut validation,
                            );
                            wasm_rpc_dependencies.push(wasm_rpc.component_name)
                        }
                        Err(err) => validation
                            .add_error(format!("Failed to get wasm-rpc trait properties: {}", err)),
                    }

                    validation.pop_context();
                }

                wasm_rpc_dependencies
                    .iter()
                    .counts()
                    .into_iter()
                    .filter(|(_, count)| *count > 1)
                    .for_each(|(dep_component_name, count)| {
                        validation.add_warn(
                            format!("WASM RPC dependency specified multiple times for component: {}, count: {}", dep_component_name, count)
                        );
                    });

                let wasm_rpc_dependencies = wasm_rpc_dependencies
                    .into_iter()
                    .unique()
                    .sorted()
                    .collect::<Vec<_>>();

                if !validation.has_any_errors() {
                    if let Ok(properties) = properties {
                        properties.add_unknown_property_warns(Vec::new, &mut validation);

                        components.push(Component {
                            name: component.name,
                            component_type,
                            source: application.source.clone(),
                            wit: properties.wit.into(),
                            input_wasm: properties.input_wasm.into(),
                            output_wasm: properties.output_wasm.into(),
                            wasm_rpc_dependencies,
                        })
                    }
                }

                validation.pop_context();
                validation.pop_context();
            }

            for component in application.application.spec.components {
                validation.push_context("component name", component.name.clone());
                validation.push_context("component type", component.component_type.clone());

                validation.add_warn("Unknown component-type".to_string());

                validation.pop_context();
                validation.pop_context();
            }
        }

        validation.build(components)
    }

    pub fn from_oam_applications(
        applications: Vec<oam::ApplicationWithSource>,
    ) -> ValidatedResult<Vec<Component>> {
        let mut result = ValidatedResult::Ok(vec![]);

        for app in applications {
            result = result.combine(Component::from_oam_application(app), |mut a, b| {
                a.extend(b);
                a
            });
        }

        result
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

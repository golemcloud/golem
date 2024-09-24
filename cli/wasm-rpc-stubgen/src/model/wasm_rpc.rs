use crate::model::oam;
use crate::model::validation::{ValidationBuilder, ValidationResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub type Result<T> = std::result::Result<(T, Vec<String>), Vec<String>>;

pub const TRAIT_TYPE_WASM_RPC: &str = "wasm-rpc";

pub const COMPONENT_TYPE_DURABLE: &str = "durable";
pub const COMPONENT_TYPE_EPHEMERAL: &str = "ephemeral";

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
pub struct ComponentProperties {
    pub wit: String,
    #[serde(rename = "inputWasm")]
    pub input_wasm: String,
    #[serde(rename = "outputWasm")]
    pub output_wasm: String,
    #[serde(flatten)]
    pub extra_properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraitWasmRpcProperties {
    #[serde(rename = "componentName")]
    pub component_name: String,
    #[serde(flatten)]
    pub extra_properties: BTreeMap<String, serde_json::Value>,
}

impl Component {
    pub fn from_oam_application(
        mut application: oam::ApplicationWithSource,
    ) -> Result<Vec<Component>> {
        let mut validation = ValidationBuilder::new();
        validation.push_context("source", application.source.to_string_lossy().to_string());

        let mut components = Vec::<Component>::new();

        if application.application.spec.components.is_empty() {
            validation.add_error("Expected at least one component specification".to_string());
        } else {
            let mut components_by_type = application.extract_components_by_type(&BTreeSet::from([
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

                let properties = component.clone_properties_as::<ComponentProperties>();
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

                    match oam::TypedTrait::<TraitWasmRpcProperties>::try_from(wasm_rpc) {
                        Ok(wasm_rpc) => {
                            if !wasm_rpc.properties.extra_properties.is_empty() {
                                validation.push_context(
                                    "dep component name",
                                    wasm_rpc.properties.component_name.clone(),
                                );

                                for (name, _) in wasm_rpc.properties.extra_properties {
                                    validation.add_warn(format!(
                                        "Unknown wasm-rpc trait property: {}",
                                        name
                                    ));
                                }

                                validation.pop_context();
                            }
                            wasm_rpc_dependencies.push(wasm_rpc.properties.component_name)
                        }
                        Err(err) => validation
                            .add_error(format!("Failed to get wasm-rpc trait properties: {}", err)),
                    }

                    validation.pop_context();
                }

                if !validation.has_any_errors() {
                    if let Ok(properties) = properties {
                        if !properties.extra_properties.is_empty() {
                            for (name, _) in properties.extra_properties {
                                validation
                                    .add_warn(format!("Unknown component property: {}", name));
                            }
                        }

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

        match validation.build() {
            ValidationResult::Ok => Ok((components, vec![])),
            ValidationResult::Warns(warns) => Ok((components, warns)),
            ValidationResult::WarnsAndErrors(warns_and_errors) => Err(warns_and_errors),
        }
    }

    pub fn from_oam_applications(
        applications: Vec<oam::ApplicationWithSource>,
    ) -> Result<Vec<Component>> {
        let mut validation_result = ValidationResult::Ok;
        let mut components = Vec::<Component>::new();

        for app in applications {
            match Component::from_oam_application(app) {
                Ok((app_components, warns)) => {
                    components.extend(app_components);
                    validation_result = validation_result.merge(ValidationResult::Warns(warns));
                }
                Err(warns_and_errors) => {
                    validation_result =
                        validation_result.merge(ValidationResult::WarnsAndErrors(warns_and_errors))
                }
            }
        }

        match validation_result {
            ValidationResult::Ok => Ok((components, vec![])),
            ValidationResult::Warns(warns) => Ok((components, warns)),
            ValidationResult::WarnsAndErrors(warns_and_errors) => Err(warns_and_errors),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::oam::Application;
    use assert2::assert;

    #[test]
    fn deserialize_example_application() {
        let application: Application = serde_yaml::from_str(
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
"#,
        )
        .unwrap();

        assert!(application.api_version == oam::API_VERSION_V1BETA1);
        assert!(application.kind == oam::KIND_APPLICATION);
        assert!(application.metadata.name == "App name");
        assert!(application.spec.components.len() == 2);

        let components = Component::from_oam_application(oam::ApplicationWithSource {
            source: PathBuf::from("test"),
            application,
        });

        assert!(components.as_ref().err() == None);

        let (components, warns) = components.unwrap();

        println!("{}", warns.join("\n"));

        assert!(warns.len() == 3);
        assert!(components.len() == 1);

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
}

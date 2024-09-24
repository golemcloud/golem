use crate::model::oam;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Vec<String>>;

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
    pub worker_rpc_dependencies: Vec<String>,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraitWasmRpcProperties {
    #[serde(rename = "componentName")]
    pub component_name: String,
}

impl Component {
    pub fn from_oam_application(application: oam::ApplicationWithSource) -> Result<Vec<Component>> {
        let mut components = Vec::<Component>::new();
        let mut errors = Vec::<String>::new();
        let mut add_error = |err: String| {
            errors.push(format!(
                "Error in {}: {}",
                application.source.to_string_lossy(),
                err
            ))
        };

        if application.application.spec.components.is_empty() {
            add_error("Expected at least one component specification".to_string());
        } else {
            'component: for component in application.application.spec.components {
                let mut add_component_error = |err: String| {
                    add_error(format!(
                        "Error in component ({}) specification: {}",
                        component.name, err
                    ));
                };
                let log_skip_reason = |reason: String| {
                    eprintln!(
                        "{} (component: {}, source: {})",
                        reason,
                        component.name,
                        application.source.to_string_lossy()
                    );
                };

                let component_type =
                    match ComponentType::try_from(component.component_type.as_str()) {
                        Ok(component_type) => Some(component_type),
                        Err(err) => {
                            log_skip_reason(format!("Skipping component: {}", err));
                            continue 'component;
                        }
                    };

                let properties =
                    match serde_json::from_value::<ComponentProperties>(component.properties) {
                        Ok(properties) => Some(properties),
                        Err(err) => {
                            add_component_error(format!(
                                "Failed to get component properties: {}",
                                err
                            ));
                            None
                        }
                    };

                let worker_rpc_dependencies = {
                    let mut worker_rpc_dependencies = Vec::<String>::new();
                    let mut has_errors = false;
                    for component_trait in component.traits {
                        let properties = match component_trait.trait_type.as_str() {
                            TRAIT_TYPE_WASM_RPC => {
                                match serde_json::from_value::<TraitWasmRpcProperties>(
                                    component_trait.properties,
                                ) {
                                    Ok(properties) => Some(properties),
                                    Err(err) => {
                                        add_component_error(format!(
                                            "Failed to get WASM RPC trait properties: {}",
                                            err
                                        ));
                                        has_errors = true;
                                        None
                                    }
                                }
                            }
                            other => {
                                log_skip_reason(format!("Skipping unknown trait: {}", other));
                                None
                            }
                        };
                        if let Some(properties) = properties {
                            worker_rpc_dependencies.push(properties.component_name);
                        }
                    }

                    (!has_errors).then_some(worker_rpc_dependencies)
                };

                if let (Some(component_type), Some(properties), Some(worker_rpc_dependencies)) =
                    (component_type, properties, worker_rpc_dependencies)
                {
                    components.push(Component {
                        name: component.name,
                        component_type,
                        source: application.source.clone(),
                        wit: properties.wit.into(),
                        input_wasm: properties.input_wasm.into(),
                        output_wasm: properties.output_wasm.into(),
                        worker_rpc_dependencies,
                    })
                }
            }
        }

        if errors.is_empty() {
            Ok(components)
        } else {
            Err(errors)
        }
    }

    pub fn from_oam_applications(
        applications: Vec<oam::ApplicationWithSource>,
    ) -> Result<Vec<Component>> {
        let mut all_components = Vec::<Component>::new();
        let mut errors = Vec::<String>::new();

        for app in applications {
            match Component::from_oam_application(app) {
                Ok(app_components) => all_components.extend(app_components),
                Err(errs) => errors.extend(errs),
            }
        }

        if errors.is_empty() {
            Ok(all_components)
        } else {
            Err(errors)
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
      traits:
        - type: wasm-rpc
          properties:
            componentName: component-two
        - type: wasm-rpc
          properties:
            componentName: component-three
        - type: unknown-trait
          properties:
            unknownProp: test
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

        let components = components.unwrap();

        assert!(components.len() == 1);

        let component = &components[0];

        assert!(component.name == "component-one");
        assert!(component.component_type == ComponentType::Durable);
        assert!(component.wit.to_string_lossy() == "wit");
        assert!(component.input_wasm.to_string_lossy() == "out/in.wasm");
        assert!(component.output_wasm.to_string_lossy() == "out/out.wasm");
        assert!(component.worker_rpc_dependencies.len() == 2);

        assert!(component.worker_rpc_dependencies[0] == "component-two");
        assert!(component.worker_rpc_dependencies[1] == "component-three");
    }
}

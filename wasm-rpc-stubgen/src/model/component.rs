use crate::model::oam;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Vec<String>>;

const TRAIT_TYPE_WORKER_RPC: &str = "worker-rpc";

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    // TODO: component type
    pub source: PathBuf,
    pub wit: PathBuf,
    pub wasm: PathBuf,
    pub composed_wasm: PathBuf,
    pub worker_rpc_dependencies: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentProperties {
    pub wit: String,
    pub wasm: String,
    #[serde(rename = "composedWasm")]
    pub composed_wasm: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraitWorkerRpcProperties {
    #[serde(rename = "componentName")]
    pub component_name: String,
}

impl Component {
    pub fn from_oam_application(application: oam::ApplicationWithSource) -> Result<Vec<Component>> {
        let mut components = Vec::<Component>::new();
        let mut errors = Vec::<String>::new();
        let mut add_error = |err: String| {
            errors.push(format!("Error in {}: {}", application.source.to_string_lossy(), err))
        };

        if application.application.spec.components.is_empty() {
            add_error("Expected at least one component specification".to_string());
        } else {
            for component in application.application.spec.components {
                let mut add_component_error = |err: String| {
                    add_error(format!("Error in component ({}) specification: {}", component.name, err));
                };

                let properties = match serde_json::from_value::<ComponentProperties>(component.properties) {
                    Ok(properties) => Some(properties),
                    Err(err) => {
                        add_component_error(format!("Failed to get component properties: {}", err));
                        None
                    }
                };

                let worker_rpc_dependencies = {
                    let mut worker_rpc_dependencies = Vec::<String>::new();
                    let mut has_errors = false;
                    for component_trait in component.traits {
                        let properties = match component_trait.trait_type.as_str() {
                            TRAIT_TYPE_WORKER_RPC => {
                                match serde_json::from_value::<TraitWorkerRpcProperties>(component_trait.properties) {
                                    Ok(properties) => Some(properties),
                                    Err(err) => {
                                        add_component_error(format!("Failed to get worker RPC trait properties: {}", err));
                                        has_errors = true;
                                        None
                                    }
                                }
                            }
                            other => {
                                eprintln!("Skipping unknown trait: {}", other);
                                None
                            }
                        };
                        if let Some(properties) = properties {
                            worker_rpc_dependencies.push(properties.component_name);
                        }
                    }

                    (!has_errors).then(|| worker_rpc_dependencies)
                };

                if let (Some(properties), Some(worker_rpc_dependencies)) = (properties, worker_rpc_dependencies) {
                    components.push(Component {
                        name: component.name,
                        source: application.source.clone(),
                        wit: properties.wit.into(),
                        wasm: properties.wasm.into(),
                        composed_wasm: properties.composed_wasm.into(),
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

    pub fn from_oam_applications(applications: Vec<oam::ApplicationWithSource>) -> Result<Vec<Component>> {
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
    use assert2::{assert};
    use crate::model::oam::Application;

    #[test]
    fn deserialize_example_application() {
        let application: Application = serde_yaml::from_str(r#"
apiVersion: core.oam.dev/v1beta1
metadata:
  name: "App name"
kind: Application
spec:
  components:
    - name: component-one
      type: component-durable
      properties:
        wit: wit
        wasm: out/component.wasm
        composedWasm: out/component-composed.wasm
      traits:
        - type: worker-rpc
          properties:
            componentName: component-two
        - type: worker-rpc
          properties:
            componentName: component-three
"#).unwrap();

        assert!(application.api_version == oam::API_VERSION_V1BETA1);
        assert!(application.kind == oam::KIND_APPLICATION);
        assert!(application.metadata.name == "App name");
        assert!(application.spec.components.len() == 1);

        let components = Component::from_oam_application(oam::ApplicationWithSource {
            source: PathBuf::from("test"),
            application,
        }).unwrap();

        assert!(components.len() == 1);

        let component = &components[0];

        assert!(component.name == "component-one");
        assert!(component.wit.to_string_lossy() == "wit");
        assert!(component.wasm.to_string_lossy() == "out/component.wasm");
        assert!(component.composed_wasm.to_string_lossy() == "out/component-composed.wasm");
        assert!(component.worker_rpc_dependencies.len() == 2);

        assert!(component.worker_rpc_dependencies[0] == "component-two");
        assert!(component.worker_rpc_dependencies[1] == "component-three");
    }
}
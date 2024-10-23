use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::constraint::FunctionUsage;
use golem_common::model::{ComponentId, ComponentType};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use rib::{RegistryKey, WorkerFunctionsInRib};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component<Namespace> {
    pub namespace: Namespace,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentConstraints<Namespace> {
    pub namespace: Namespace,
    pub component_id: ComponentId,
    pub constraints: FunctionUsageCollection,
}

// This is very similar to WorkerFunctionsInRib data structure, however
// it adds the total number of usages for each function in that component
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionUsageCollection {
    pub function_usages: Vec<FunctionUsage>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionUsageCollection> for FunctionUsageCollection {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::component::FunctionUsageCollection) -> Result<Self, Self::Error> {
        let collection = FunctionUsageCollection {
            function_usages: value.constraints.iter().map(|x| FunctionUsage::try_from(x.clone())).collect::<Result<_, _>>()?
        };

        Ok(collection)
    }
}

impl From<FunctionUsageCollection> for golem_api_grpc::proto::golem::component::FunctionUsageCollection {
    fn from(value: FunctionUsageCollection) -> Self {
        golem_api_grpc::proto::golem::component::FunctionUsageCollection {
            constraints: value.function_usages.iter().map(|x| golem_api_grpc::proto::golem::component::FunctionUsage::from(x.clone())).collect(),
        }
    }
}

impl From<FunctionUsageCollection> for WorkerFunctionsInRib {
    fn from(value: FunctionUsageCollection) -> Self {
        WorkerFunctionsInRib {
            function_calls: value.function_usages.iter().map(|x| rib::WorkerFunctionInRibMetadata::from(x.clone())).collect()
        }
    }
}

impl FunctionUsageCollection {
    pub fn try_merge(
        worker_functions: Vec<FunctionUsageCollection>,
    ) -> Result<FunctionUsageCollection, String> {
        let mut merged_function_calls: HashMap<RegistryKey, FunctionUsage> = HashMap::new();

        for wf in worker_functions {
            for call in wf.function_usages {
                match merged_function_calls.get_mut(&call.function_key) {
                    Some(existing_call) => {
                        // Check for parameter type conflicts
                        if existing_call.parameter_types != call.parameter_types {
                            return Err(format!(
                                "Parameter type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key,
                                existing_call.parameter_types,
                                call.parameter_types
                            ));
                        }

                        // Check for return type conflicts
                        if existing_call.return_types != call.return_types {
                            return Err(format!(
                                "Return type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key, existing_call.return_types, call.return_types
                            ));
                        }

                        // Update usage_count instead of overwriting
                        existing_call.usage_count =
                            existing_call.usage_count.saturating_add(call.usage_count);
                    }
                    None => {
                        // Insert if no conflict is found
                        merged_function_calls.insert(call.function_key.clone(), call);
                    }
                }
            }
        }

        let mut merged_function_calls_vec: Vec<FunctionUsage> =
            merged_function_calls.into_values().collect();

        merged_function_calls_vec.sort_by(|a, b| a.function_key.cmp(&b.function_key));

        Ok(FunctionUsageCollection {
            function_usages: merged_function_calls_vec,
        })
    }
}

impl<Namespace: Clone> ComponentConstraints<Namespace> {
    pub fn init(
        namespace: &Namespace,
        component_id: &ComponentId,
        worker_functions_in_rib: WorkerFunctionsInRib,
    ) -> ComponentConstraints<Namespace> {
        ComponentConstraints {
            namespace: namespace.clone(),
            component_id: component_id.clone(),
            constraints: FunctionUsageCollection {
                function_usages: worker_functions_in_rib
                    .function_calls
                    .iter()
                    .map(|x| FunctionUsage::from_worker_function_in_rib(x))
                    .collect(),
            },
        }
    }

    pub fn update_with(
        &self,
        function_usages: &FunctionUsageCollection,
    ) -> Result<ComponentConstraints<Namespace>, String> {
        let function_usage_collection =
            FunctionUsageCollection::try_merge(vec![self.constraints.clone(), function_usages.clone()])?;
        let component_constraints = ComponentConstraints {
            namespace: self.namespace.clone(),
            component_id: self.component_id.clone(),
            constraints: function_usage_collection,
        };

        Ok(component_constraints)
    }
}

impl<Namespace> Component<Namespace> {
    pub fn next_version(self) -> Self {
        let new_version = VersionedComponentId {
            component_id: self.versioned_component_id.component_id,
            version: self.versioned_component_id.version + 1,
        };
        Self {
            versioned_component_id: new_version.clone(),
            ..self
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_service_base::model::Component {
    fn from(value: Component<Namespace>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: Some(value.created_at),
            component_type: Some(value.component_type),
        }
    }
}

impl<Namespace> From<Component<Namespace>> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component<Namespace>) -> Self {
        let component_type: golem_api_grpc::proto::golem::component::ComponentType =
            value.component_type.into();
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            project_id: None,
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
            component_type: Some(component_type.into()),
        }
    }
}

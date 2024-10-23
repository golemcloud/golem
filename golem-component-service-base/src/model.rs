use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::component_constraint::{FunctionUsage, FunctionUsageCollection};
use golem_common::model::{ComponentId, ComponentType};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use rib::WorkerFunctionsInRib;
use serde::{Deserialize, Serialize};
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
                    .map(FunctionUsage::from_worker_function_in_rib)
                    .collect(),
            },
        }
    }

    pub fn update_with(
        &self,
        function_usages: &FunctionUsageCollection,
    ) -> Result<ComponentConstraints<Namespace>, String> {
        let function_usage_collection = FunctionUsageCollection::try_merge(vec![
            self.constraints.clone(),
            function_usages.clone(),
        ])?;
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

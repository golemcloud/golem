use chrono::Utc;
use golem_common::model::component_constraint::{FunctionConstraint, FunctionConstraintCollection};
use golem_common::model::component_metadata::{ComponentMetadata, ComponentProcessingError};
use golem_common::model::{ComponentId, ComponentType};
use golem_common::model::{InitialComponentFile, InitialComponentFilePathAndPermissionsList};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use rib::WorkerFunctionsInRib;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component<Namespace> {
    pub namespace: Namespace,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>
}

impl<Namespace> Component<Namespace> {
    pub fn new(
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: &[u8],
        namespace: &Namespace,
        files: Vec<InitialComponentFile>,
    ) -> Result<Component<Namespace>, ComponentProcessingError>
    where
        Namespace: Eq + Clone + Send + Sync,
    {
        let metadata = ComponentMetadata::analyse_component(data)?;

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: 0,
        };

        Ok(Component {
            namespace: namespace.clone(),
            component_name: component_name.clone(),
            component_size: data.len() as u64,
            metadata,
            created_at: Utc::now(),
            versioned_component_id,
            component_type,
            files
        })
    }

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
            files: value.files
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
            files: value.files.into_iter().map(|file| file.into()).collect()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentConstraints<Namespace> {
    pub namespace: Namespace,
    pub component_id: ComponentId,
    pub constraints: FunctionConstraintCollection,
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
            constraints: FunctionConstraintCollection {
                function_constraints: worker_functions_in_rib
                    .function_calls
                    .iter()
                    .map(FunctionConstraint::from_worker_function_type)
                    .collect(),
            },
        }
    }

    pub fn update_with(
        &self,
        function_constraints: &FunctionConstraintCollection,
    ) -> Result<ComponentConstraints<Namespace>, String> {
        let constraints = FunctionConstraintCollection::try_merge(vec![
            self.constraints.clone(),
            function_constraints.clone(),
        ])?;
        let component_constraints = ComponentConstraints {
            namespace: self.namespace.clone(),
            component_id: self.component_id.clone(),
            constraints,
        };

        Ok(component_constraints)
    }

}

#[derive(Debug)]
pub struct InitialComponentFilesArchiveAndPermissions {
    pub archive: File,
    pub permissions: InitialComponentFilePathAndPermissionsList,
}

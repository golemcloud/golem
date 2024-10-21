use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::{ComponentId, ComponentType};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_wasm_ast::analysis::AnalysedType;
use rib::ParsedFunctionName;
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
pub struct ComponentConstraint<Namespace> {
    pub namespace: Namespace,
    pub component_id: ComponentId,
    pub constraints: FunctionConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionConstraints {
    pub constraints: Vec<FunctionConstraint>,
}

impl From<FunctionConstraints> for golem_api_grpc::proto::golem::component::FunctionConstraints {
    fn from(value: FunctionConstraints) -> Self {
        Self {
            constraints: value
                .constraints
                .into_iter()
                .map(|function_detail| function_detail.into())
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraints> for FunctionConstraints {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionConstraints,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            constraints: value
                .constraints
                .into_iter()
                .map(|function_constraint| FunctionConstraint::try_from(function_constraint))
                .collect::<Result<_, _>>()?
        })
    }
}


// A trimmed down version of component metadata that just includes enough details
// on function calls, and is part of a component constraint
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionConstraint {
    pub function_name: ParsedFunctionName,
    pub argument_types: Vec<AnalysedType>,
    pub result_types: Vec<AnalysedType>,
}

impl From<FunctionConstraint> for golem_api_grpc::proto::golem::component::FunctionConstraint {
    fn from(value: FunctionConstraint) -> Self {
        Self {
            function_name: value.function_name.to_string(),
            argument_types: value
                .argument_types
                .iter()
                .map(|analysed_type|analysed_type.into())
                .collect(),
            result_types: value
                .result_types
                .iter()
                .map(|analysed_type| analysed_type.into())
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraint> for FunctionConstraint {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::component::FunctionConstraint) -> Result<Self, Self::Error> {
        let result = FunctionConstraint {
            function_name: ParsedFunctionName::parse(value.function_name)?,
            argument_types: value
                .argument_types
                .into_iter()
                .map(|typ| AnalysedType::try_from(&typ))
                .collect::<Result<_, _>>()?,
            result_types: value.result_types
                .into_iter()
                .map(|typ| AnalysedType::try_from(&typ))
                .collect::<Result<_, _>>()?,
        };

        Ok(result)
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

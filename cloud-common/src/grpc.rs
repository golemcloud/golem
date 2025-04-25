use crate::model::{CloudPluginOwner, CloudPluginScope, ProjectGrantId, ProjectPolicyId, TokenId};
use golem_api_grpc::proto::golem::common;
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::ProjectId;

pub fn proto_project_id_string(id: &Option<common::ProjectId>) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<ProjectId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_project_policy_id_string(
    id: &Option<cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicyId>,
) -> Option<String> {
    id.clone()
        .and_then(|v| TryInto::<ProjectPolicyId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_project_grant_id_string(
    id: &Option<cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantId>,
) -> Option<String> {
    id.clone()
        .and_then(|v| TryInto::<ProjectGrantId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_token_id_string(
    id: &Option<cloud_api_grpc::proto::golem::cloud::token::TokenId>,
) -> Option<String> {
    id.clone()
        .and_then(|v| TryInto::<TokenId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn try_decode_plugin_definition(
    value: cloud_api_grpc::proto::golem::cloud::component::PluginDefinition,
) -> Result<PluginDefinition<CloudPluginOwner, CloudPluginScope>, String> {
    Ok(PluginDefinition {
        id: value.id.ok_or("Missing plugin id")?.try_into()?,
        name: value.name,
        version: value.version,
        description: value.description,
        icon: value.icon,
        homepage: value.homepage,
        specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
        scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
        owner: CloudPluginOwner {
            account_id: value.account_id.ok_or("Missing plugin owner")?.into(),
        },
        deleted: value.deleted,
    })
}

// NOTE: Can't define a `From` instance because the gRPC type is defined in `cloud-api-grpc` and the model is defined in `golem-component-service-base`
pub fn plugin_definition_to_grpc(
    plugin_definition: PluginDefinition<CloudPluginOwner, CloudPluginScope>,
) -> cloud_api_grpc::proto::golem::cloud::component::PluginDefinition {
    cloud_api_grpc::proto::golem::cloud::component::PluginDefinition {
        id: Some(plugin_definition.id.into()),
        name: plugin_definition.name,
        version: plugin_definition.version,
        scope: Some(plugin_definition.scope.into()),
        account_id: Some(plugin_definition.owner.account_id.into()),
        description: plugin_definition.description,
        icon: plugin_definition.icon,
        homepage: plugin_definition.homepage,
        specs: Some(plugin_definition.specs.into()),
        deleted: plugin_definition.deleted,
    }
}

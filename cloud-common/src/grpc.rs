use crate::model::{ProjectGrantId, ProjectPolicyId, TokenId};
use golem_api_grpc::proto::golem::common;
use golem_common::model::ProjectId;

pub fn proto_project_id_string(id: &Option<common::ProjectId>) -> Option<String> {
    id.clone()
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

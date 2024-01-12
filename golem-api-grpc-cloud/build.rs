use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    tonic_build::configure()
        .extern_path(".golem.common", "::golem_api_grpc::proto::golem::common")
        .extern_path(
            ".golem.template",
            "::golem_api_grpc::proto::golem::template",
        )
        .extern_path(
            ".golem.shardmanager",
            "::golem_api_grpc::proto::golem::shardmanager",
        )
        .extern_path(".golem.worker", "::golem_api_grpc::proto::golem::worker")
        .extern_path(
            ".golem.workerexecutor",
            "::golem_api_grpc::proto::golem::workerexecutor",
        )
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .type_attribute(
            ".",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .include_file("mod.rs")
        .compile(
            &[
                "proto/golem/cloud/account/account_service.proto",
                "proto/golem/cloud/account/account.proto",
                "proto/golem/cloud/account/account_data.proto",
                "proto/golem/cloud/account/account_error.proto",
                "proto/golem/cloud/accountsummary/account_summary.proto",
                "proto/golem/cloud/accountsummary/account_summary_error.proto",
                "proto/golem/cloud/accountsummary/account_summary_service.proto",
                "proto/golem/cloud/grant/grant_service.proto",
                "proto/golem/cloud/grant/grant_error.proto",
                "proto/golem/cloud/grant/role.proto",
                "proto/golem/cloud/plan/plan.proto",
                "proto/golem/cloud/plan/plan_data.proto",
                "proto/golem/cloud/plan/plan_id.proto",
                "proto/golem/cloud/limit/batch_update_resource_limits.proto",
                "proto/golem/cloud/limit/limits_error.proto",
                "proto/golem/cloud/limit/limits_service.proto",
                "proto/golem/cloud/login/login_error.proto",
                "proto/golem/cloud/login/o_auth2_data.proto",
                "proto/golem/cloud/login/login_service.proto",
                "proto/golem/cloud/projectgrant/project_grant_id.proto",
                "proto/golem/cloud/projectgrant/project_grant.proto",
                "proto/golem/cloud/projectgrant/project_grant_data.proto",
                "proto/golem/cloud/projectgrant/project_grant_data_request.proto",
                "proto/golem/cloud/projectgrant/project_grant_error.proto",
                "proto/golem/cloud/projectgrant/project_grant_service.proto",
                "proto/golem/cloud/projectpolicy/project_action.proto",
                "proto/golem/cloud/projectpolicy/project_actions.proto",
                "proto/golem/cloud/projectpolicy/project_policy.proto",
                "proto/golem/cloud/projectpolicy/project_policy_data.proto",
                "proto/golem/cloud/projectpolicy/project_policy_error.proto",
                "proto/golem/cloud/projectpolicy/project_policy_id.proto",
                "proto/golem/cloud/projectpolicy/project_policy_service.proto",
                "proto/golem/cloud/project/project_id.proto",
                "proto/golem/cloud/project/project.proto",
                "proto/golem/cloud/project/project_data.proto",
                "proto/golem/cloud/project/project_data_request.proto",
                "proto/golem/cloud/project/project_error.proto",
                "proto/golem/cloud/project/project_type.proto",
                "proto/golem/cloud/project/project_service.proto",
                "proto/golem/cloud/template/template.proto",
                "proto/golem/cloud/template/template_service.proto",
                "proto/golem/cloud/token/create_token_dto.proto",
                "proto/golem/cloud/token/token.proto",
                "proto/golem/cloud/token/token_error.proto",
                "proto/golem/cloud/token/token_id.proto",
                "proto/golem/cloud/token/token_secret.proto",
                "proto/golem/cloud/token/unsafe_token.proto",
                "proto/golem/cloud/token/token_service.proto",
            ],
            &["../golem-api-grpc/proto", "proto"],
        )
        .unwrap();

    Ok(())
}

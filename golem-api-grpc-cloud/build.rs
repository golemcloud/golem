use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    tonic_build::configure()
        .extern_path(".golem.common", "::golem_api_grpc::proto::golem::common")
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .type_attribute(
            ".",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .include_file("mod.rs")
        .compile(
            &[
                "proto/golem/cloud/common/account.proto",
                "proto/golem/cloud/common/account_data.proto",
                "proto/golem/cloud/common/account_error.proto",
                "proto/golem/cloud/common/account_summary.proto",
                "proto/golem/cloud/common/account_summary_error.proto",
                "proto/golem/cloud/common/batch_update_resource_limits.proto",
                "proto/golem/cloud/common/create_token_dto.proto",
                "proto/golem/cloud/common/grant_error.proto",
                "proto/golem/cloud/common/grant_id.proto",
                "proto/golem/cloud/common/limits_error.proto",
                "proto/golem/cloud/common/login_error.proto",
                "proto/golem/cloud/common/o_auth2_data.proto",
                "proto/golem/cloud/common/plan.proto",
                "proto/golem/cloud/common/plan_data.proto",
                "proto/golem/cloud/common/plan_id.proto",
                "proto/golem/cloud/common/project_id.proto",
                "proto/golem/cloud/common/project.proto",
                "proto/golem/cloud/common/project_action.proto",
                "proto/golem/cloud/common/project_actions.proto",
                "proto/golem/cloud/common/project_data.proto",
                "proto/golem/cloud/common/project_data_request.proto",
                "proto/golem/cloud/common/project_error.proto",
                "proto/golem/cloud/common/project_grant.proto",
                "proto/golem/cloud/common/project_grant_data.proto",
                "proto/golem/cloud/common/project_grant_data_request.proto",
                "proto/golem/cloud/common/project_grant_error.proto",
                "proto/golem/cloud/common/project_policy.proto",
                "proto/golem/cloud/common/project_policy_data.proto",
                "proto/golem/cloud/common/project_policy_error.proto",
                "proto/golem/cloud/common/project_policy_id.proto",
                "proto/golem/cloud/common/project_type.proto",
                "proto/golem/cloud/common/role.proto",
                "proto/golem/cloud/common/template.proto",
                "proto/golem/cloud/common/token.proto",
                "proto/golem/cloud/common/token_error.proto",
                "proto/golem/cloud/common/token_id.proto",
                "proto/golem/cloud/common/token_secret.proto",
                "proto/golem/cloud/common/unsafe_token.proto",
                "proto/golem/cloud/account/account_service.proto",
                "proto/golem/cloud/accountsummary/account_summary_service.proto",
                "proto/golem/cloud/grant/grant_service.proto",
                "proto/golem/cloud/limit/limits_service.proto",
                "proto/golem/cloud/login/login_service.proto",
                "proto/golem/cloud/projectgrant/project_grant_service.proto",
                "proto/golem/cloud/projectpolicy/project_policy_service.proto",
                "proto/golem/cloud/project/project_service.proto",
                "proto/golem/cloud/template/template_service.proto",
                "proto/golem/cloud/token/token_service.proto",
            ],
            &["../golem-api-grpc/proto", "proto"],
        )
        .unwrap();

    Ok(())
}

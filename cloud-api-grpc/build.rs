use cargo_metadata::MetadataCommand;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let golem_api_grpc_root = find_package_root("golem-api-grpc");

    tonic_build::configure()
        .extern_path(".wasm.rpc", "::golem_wasm_rpc::protobuf")
        .extern_path(".golem.common", "::golem_api_grpc::proto::golem::common")
        .extern_path(
            ".golem.component",
            "::golem_api_grpc::proto::golem::component",
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
        .extern_path(
            ".golem.componentcompilation",
            "::golem_api_grpc::proto::golem::componentcompilation",
        )
        .extern_path(
            ".golem.apidefinition",
            "::golem_api_grpc::proto::golem::apidefinition",
        )
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .type_attribute(
            "golem.worker.LogEvent",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "golem.worker.LogEvent.event",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "golem.worker.StdOutLog",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "golem.worker.StdErrLog",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "golem.worker.Level",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "golem.worker.Log",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .include_file("mod.rs")
        .compile(
            &[
                "proto/golem/cloud/account/account.proto",
                "proto/golem/cloud/account/account_data.proto",
                "proto/golem/cloud/account/v1/account_error.proto",
                "proto/golem/cloud/account/v1/account_service.proto",
                "proto/golem/cloud/accountsummary/v1/account_summary.proto",
                "proto/golem/cloud/accountsummary/v1/account_summary_error.proto",
                "proto/golem/cloud/accountsummary/v1/account_summary_service.proto",
                "proto/golem/cloud/auth/v1/auth_service.proto",
                "proto/golem/cloud/auth/v1/auth_error.proto",
                "proto/golem/cloud/component/plugin_definition.proto",
                "proto/golem/cloud/component/plugin_scope.proto",
                "proto/golem/cloud/component/v1/plugin_service.proto",
                "proto/golem/cloud/plan/plan.proto",
                "proto/golem/cloud/plan/plan_data.proto",
                "proto/golem/cloud/plan/plan_id.proto",
                "proto/golem/cloud/limit/v1/batch_update_resource_limits.proto",
                "proto/golem/cloud/limit/v1/limits_error.proto",
                "proto/golem/cloud/limit/v1/limits_service.proto",
                "proto/golem/cloud/login/v1/login_error.proto",
                "proto/golem/cloud/login/o_auth2_data.proto",
                "proto/golem/cloud/login/v1/login_service.proto",
                "proto/golem/cloud/projectgrant/project_grant_id.proto",
                "proto/golem/cloud/projectpolicy/project_action.proto",
                "proto/golem/cloud/projectpolicy/project_policy_id.proto",
                "proto/golem/cloud/project/project.proto",
                "proto/golem/cloud/project/project_data.proto",
                "proto/golem/cloud/project/v1/project_error.proto",
                "proto/golem/cloud/project/project_type.proto",
                "proto/golem/cloud/project/v1/project_service.proto",
                "proto/golem/cloud/token/create_token_dto.proto",
                "proto/golem/cloud/token/token.proto",
                "proto/golem/cloud/token/v1/token_error.proto",
                "proto/golem/cloud/token/token_id.proto",
                "proto/golem/cloud/token/token_secret.proto",
                "proto/golem/cloud/token/unsafe_token.proto",
                "proto/golem/cloud/token/v1/token_service.proto",
            ],
            &[
                &format!("{golem_api_grpc_root}/proto"),
                &"proto".to_string(),
            ],
        )
        .unwrap();

    Ok(())
}

fn find_package_root(name: &str) -> String {
    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .unwrap();
    let package = metadata.packages.iter().find(|p| p.name == name).unwrap();
    package.manifest_path.parent().unwrap().to_string()
}

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
                "proto/golem/cloud/account/account_service.proto",
                "proto/golem/cloud/accountsummary/account_summary_service.proto",
                "proto/golem/cloud/grant/grant_service.proto",
                "proto/golem/cloud/limit/limits_service.proto",
                "proto/golem/cloud/login/login_service.proto",
                "proto/golem/cloud/projectgrant/project_grant_service.proto",
                "proto/golem/cloud/projectpolicy/project_policy_service.proto",
                "proto/golem/cloud/project/project_service.proto",
                "proto/golem/cloud/token/token_service.proto",
            ],
            &["../golem-api-grpc/proto", "proto"],
        )
        .unwrap();

    Ok(())
}

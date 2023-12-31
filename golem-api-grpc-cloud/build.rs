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
                "proto/golem/cloudservices/accountservice/account_service.proto",
                "proto/golem/cloudservices/accountsummaryservice/account_summary_service.proto",
                "proto/golem/cloudservices/grantservice/grant_service.proto",
                "proto/golem/cloudservices/limitsservice/limits_service.proto",
                "proto/golem/cloudservices/loginservice/login_service.proto",
                "proto/golem/cloudservices/projectgrantservice/project_grant_service.proto",
                "proto/golem/cloudservices/projectpolicyservice/project_policy_service.proto",
                "proto/golem/cloudservices/projectservice/project_service.proto",
                "proto/golem/cloudservices/tokenservice/token_service.proto",
            ],
            &["../golem-api-grpc/proto", "proto"],
        )
        .unwrap();

    Ok(())
}

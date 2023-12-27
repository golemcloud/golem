use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .type_attribute(
            ".",
            "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
        )
        .include_file("mod.rs")
        .compile(
            &[
                "proto/services/account_service.proto",
                "proto/services/account_summary_service.proto",
                "proto/services/grant_service.proto",
                "proto/services/limits_service.proto",
                "proto/services/login_service.proto",
                "proto/services/project_grant_service.proto",
                "proto/services/project_policy_service.proto",
                "proto/services/project_service.proto",
                "proto/services/shard_manager_service.proto",
                "proto/services/template_service.proto",
                "proto/services/token_service.proto",
                "proto/services/worker_executor.proto",
                "proto/services/worker_service.proto",
            ],
            &["proto"],
        )
        .unwrap();

    Ok(())
}

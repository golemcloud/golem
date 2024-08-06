use cargo_metadata::MetadataCommand;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let wasm_rpc_root = find_package_root("golem-wasm-rpc");

    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .extern_path(".wasm.rpc", "::golem_wasm_rpc::protobuf")
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
                "proto/golem/rib/function_name.proto",
                "proto/golem/rib/expr.proto",
                "proto/golem/common/account_id.proto",
                "proto/golem/common/project_id.proto",
                "proto/golem/common/empty.proto",
                "proto/golem/common/error_body.proto",
                "proto/golem/common/resource_limits.proto",
                "proto/golem/common/uuid.proto",
                "proto/golem/component/producer_field.proto",
                "proto/golem/component/producers.proto",
                "proto/golem/component/protected_component_id.proto",
                "proto/golem/component/export.proto",
                "proto/golem/component/export_function.proto",
                "proto/golem/component/export_instance.proto",
                "proto/golem/component/function_parameter.proto",
                "proto/golem/component/function_result.proto",
                "proto/golem/component/component.proto",
                "proto/golem/component/component_id.proto",
                "proto/golem/component/component_metadata.proto",
                "proto/golem/component/user_component_id.proto",
                "proto/golem/component/versioned_name.proto",
                "proto/golem/component/versioned_component_id.proto",
                "proto/golem/component/v1/component_service.proto",
                "proto/golem/component/v1/component_error.proto",
                "proto/golem/componentcompilation/v1/component_compilation_service.proto",
                "proto/golem/worker/complete_parameters.proto",
                "proto/golem/worker/idempotency_key.proto",
                "proto/golem/worker/invoke_parameters.proto",
                "proto/golem/worker/invoke_result.proto",
                "proto/golem/worker/log_event.proto",
                "proto/golem/worker/promise_id.proto",
                "proto/golem/worker/worker_id.proto",
                "proto/golem/worker/worker_metadata.proto",
                "proto/golem/worker/worker_filter.proto",
                "proto/golem/worker/worker_status.proto",
                "proto/golem/worker/v1/worker_service.proto",
                "proto/golem/worker/v1/worker_execution_error.proto",
                "proto/golem/worker/v1/worker_error.proto",
                "proto/golem/workerexecutor/v1/worker_executor.proto",
                "proto/golem/shardmanager/pod.proto",
                "proto/golem/shardmanager/routing_table.proto",
                "proto/golem/shardmanager/routing_table_entry.proto",
                "proto/golem/shardmanager/shard_id.proto",
                "proto/golem/shardmanager/v1/shard_manager_error.proto",
                "proto/golem/shardmanager/v1/shard_manager_service.proto",
                "proto/golem/apidefinition/api_definition.proto",
                "proto/golem/apidefinition/v1/api_definition_service.proto",
                "proto/golem/apidefinition/v1/api_definition_error.proto",
                "proto/grpc/health/v1/health.proto",
            ],
            &[&format!("{wasm_rpc_root}/proto"), &"proto".to_string()],
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

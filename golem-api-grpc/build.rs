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
                "proto/golem/common/account_id.proto",
                "proto/golem/common/calling_convention.proto",
                "proto/golem/common/complete_parameters.proto",
                "proto/golem/common/empty.proto",
                "proto/golem/common/error_body.proto",
                "proto/golem/common/export.proto",
                "proto/golem/common/export_function.proto",
                "proto/golem/common/export_instance.proto",
                "proto/golem/common/function_parameter.proto",
                "proto/golem/common/function_result.proto",
                "proto/golem/common/golem_error.proto",
                "proto/golem/common/invocation_key.proto",
                "proto/golem/common/invoke_parameters.proto",
                "proto/golem/common/invoke_result.proto",
                "proto/golem/common/invoke_result_json.proto",
                "proto/golem/common/log_event.proto",
                "proto/golem/common/pod.proto",
                "proto/golem/common/producer_field.proto",
                "proto/golem/common/producers.proto",
                "proto/golem/common/promise_id.proto",
                "proto/golem/common/protected_template_id.proto",
                "proto/golem/common/resource_limits.proto",
                "proto/golem/common/routing_table.proto",
                "proto/golem/common/routing_table_entry.proto",
                "proto/golem/common/shard_id.proto",
                "proto/golem/common/shard_manager_error.proto",
                "proto/golem/common/template.proto",
                "proto/golem/common/template_error.proto",
                "proto/golem/common/template_id.proto",
                "proto/golem/common/template_metadata.proto",
                "proto/golem/common/type.proto",
                "proto/golem/common/user_template_id.proto",
                "proto/golem/common/uuid.proto",
                "proto/golem/common/val.proto",
                "proto/golem/common/versioned_name.proto",
                "proto/golem/common/versioned_template_id.proto",
                "proto/golem/common/versioned_worker_id.proto",
                "proto/golem/common/worker_error.proto",
                "proto/golem/common/worker_id.proto",
                "proto/golem/common/worker_metadata.proto",
                "proto/golem/common/worker_status.proto",
                "proto/golem/template/template_service.proto",
                "proto/golem/worker/worker_service.proto",
                "proto/golem/workerexecutor/worker_executor.proto",
                "proto/golem/shardmanager/shard_manager_service.proto",
            ],
            &["proto"],
        )
        .unwrap();

    Ok(())
}

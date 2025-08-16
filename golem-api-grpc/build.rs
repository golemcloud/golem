use cargo_metadata::MetadataCommand;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let wasm_rpc_root = find_package_root("golem-wasm-rpc");
    let wasm_ast_root = find_package_root("golem-wasm-ast");

    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("services.bin"))
        .extern_path(".wasm.rpc", "::golem_wasm_rpc::protobuf")
        .extern_path(".wasm.ast", "::golem_wasm_ast::analysis::protobuf")
        .include_file("mod.rs")
        .compile_protos(
            &[
                "proto/golem/account/account.proto",
                "proto/golem/account/account_data.proto",
                "proto/golem/account/v1/account_error.proto",
                "proto/golem/account/v1/account_service.proto",
                "proto/golem/apidefinition/api_definition.proto",
                "proto/golem/auth/account_action.proto",
                "proto/golem/auth/project_action.proto",
                "proto/golem/auth/v1/auth_error.proto",
                "proto/golem/auth/v1/auth_service.proto",
                "proto/golem/common/account_id.proto",
                "proto/golem/common/empty.proto",
                "proto/golem/common/error_body.proto",
                "proto/golem/common/plugin_installation_id.proto",
                "proto/golem/common/project_id.proto",
                "proto/golem/common/resource_limits.proto",
                "proto/golem/common/uuid.proto",
                "proto/golem/component/component.proto",
                "proto/golem/component/component_constraints.proto",
                "proto/golem/component/component_id.proto",
                "proto/golem/component/component_metadata.proto",
                "proto/golem/component/export.proto",
                "proto/golem/component/export_function.proto",
                "proto/golem/component/export_instance.proto",
                "proto/golem/component/function_constraint.proto",
                "proto/golem/component/function_parameter.proto",
                "proto/golem/component/function_result.proto",
                "proto/golem/component/plugin_definition.proto",
                "proto/golem/component/producer_field.proto",
                "proto/golem/component/producers.proto",
                "proto/golem/component/v1/component_error.proto",
                "proto/golem/component/v1/component_service.proto",
                "proto/golem/component/v1/plugin_service.proto",
                "proto/golem/component/versioned_component_id.proto",
                "proto/golem/component/versioned_name.proto",
                "proto/golem/componentcompilation/v1/component_compilation_service.proto",
                "proto/golem/limit/v1/batch_update_resource_limits.proto",
                "proto/golem/limit/v1/limits_error.proto",
                "proto/golem/limit/v1/limits_service.proto",
                "proto/golem/project/project.proto",
                "proto/golem/project/project_data.proto",
                "proto/golem/project/project_type.proto",
                "proto/golem/project/v1/project_error.proto",
                "proto/golem/project/v1/project_service.proto",
                "proto/golem/rib/compiler_output.proto",
                "proto/golem/rib/expr.proto",
                "proto/golem/rib/function_name.proto",
                "proto/golem/rib/instance_type.proto",
                "proto/golem/rib/ir.proto",
                "proto/golem/rib/rib_byte_code.proto",
                "proto/golem/rib/rib_input.proto",
                "proto/golem/rib/rib_output.proto",
                "proto/golem/rib/type_name.proto",
                "proto/golem/rib/worker_functions_in_rib.proto",
                "proto/golem/shardmanager/pod.proto",
                "proto/golem/shardmanager/routing_table.proto",
                "proto/golem/shardmanager/routing_table_entry.proto",
                "proto/golem/shardmanager/shard_id.proto",
                "proto/golem/shardmanager/v1/shard_manager_error.proto",
                "proto/golem/shardmanager/v1/shard_manager_service.proto",
                "proto/golem/token/create_token_dto.proto",
                "proto/golem/token/token.proto",
                "proto/golem/token/token_id.proto",
                "proto/golem/token/token_secret.proto",
                "proto/golem/token/unsafe_token.proto",
                "proto/golem/token/v1/token_error.proto",
                "proto/golem/token/v1/token_service.proto",
                "proto/golem/worker/complete_parameters.proto",
                "proto/golem/worker/idempotency_key.proto",
                "proto/golem/worker/invoke_parameters.proto",
                "proto/golem/worker/invoke_result.proto",
                "proto/golem/worker/log_event.proto",
                "proto/golem/worker/promise_id.proto",
                "proto/golem/worker/public_oplog.proto",
                "proto/golem/worker/update_mode.proto",
                "proto/golem/worker/v1/worker_error.proto",
                "proto/golem/worker/v1/worker_execution_error.proto",
                "proto/golem/worker/v1/worker_service.proto",
                "proto/golem/worker/wasi_config_vars.proto",
                "proto/golem/worker/worker_error.proto",
                "proto/golem/worker/worker_filter.proto",
                "proto/golem/worker/worker_id.proto",
                "proto/golem/worker/worker_metadata.proto",
                "proto/golem/worker/worker_status.proto",
                "proto/golem/workerexecutor/v1/worker_executor.proto",
                "proto/grpc/health/v1/health.proto",
            ],
            &[
                &format!("{wasm_rpc_root}/proto"),
                &format!("{wasm_ast_root}/proto"),
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
    let package = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == name)
        .unwrap();
    package.manifest_path.parent().unwrap().to_string()
}

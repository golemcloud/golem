use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use golem_openapi_client_generator::parse_openapi_specs;

fn main() {
    let out_dir = var_os("OUT_DIR").unwrap();

    let root_yaml_path = PathBuf::from("../openapi/golem-service.yaml");
    let local_yaml_path = PathBuf::from("openapi/golem-service.yaml");

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed={}", root_yaml_path.display());
    println!("cargo::rerun-if-changed={}", local_yaml_path.display());

    println!("Starting code generation for Golem OpenAPI client.");

    println!("Output directory: {out_dir:?}");
    println!("Workspace OpenAPI file: {root_yaml_path:?}");

    if root_yaml_path.exists() {
        // Copying the file to the crate so it gets packaged
        std::fs::create_dir_all(local_yaml_path.parent().unwrap()).unwrap();
        copy_if_different(root_yaml_path.clone(), local_yaml_path.clone()).unwrap();
    };
    generate(local_yaml_path.clone(), out_dir)
}

fn generate(yaml_path: PathBuf, out_dir: OsString) {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[yaml_path]).expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-client",
        "0.0.0",
        false,
        true,
        &[
            (
                "ComponentFileOptions",
                "golem_common::model::component::ComponentFileOptions",
            ),
            (
                "NewComponentData",
                "golem_common::model::component::NewComponentData",
            ),
            (
                "UpdatedComponentData",
                "golem_common::model::component::UpdatedComponentData",
            ),
            (
                "NewPluginRegistrationData",
                "golem_common::model::plugin_registration::NewPluginRegistrationData",
            ),
            (
                "PluginRegistrationDto",
                "golem_common::model::plugin_registration::PluginRegistrationDto",
            ),
            (
                "ComponentTransformerPluginSpec",
                "golem_common::model::plugin_registration::ComponentTransformerPluginSpec",
            ),
            ("PluginScope", "golem_common::model::plugin::PluginScope"),
            ("Account", "golem_common::model::account::Account"),
            (
                "NewAccountData",
                "golem_common::model::account::NewAccountData",
            ),
            (
                "UpdatedAccountData",
                "golem_common::model::account::UpdatedAccountData",
            ),
            ("Token", "golem_common::model::auth::Token"),
            (
                "TokenWithSecret",
                "golem_common::model::auth::TokenWithSecret",
            ),
            (
                "Application",
                "golem_common::model::application::Application",
            ),
            (
                "NewApplicationData",
                "golem_common::model::application::NewApplicationData",
            ),
            (
                "UpdatedApplicationData",
                "golem_common::model::application::UpdatedApplicationData",
            ),
            (
                "RegisteredAgentType",
                "golem_common::model::agent::RegisteredAgentType",
            ),
            ("ShardId", "golem_common::model::ShardId"),
            ("ValueAndType", "golem_wasm_rpc::ValueAndType"),
            (
                "NewEnvironmentData",
                "golem_common::model::environment::NewEnvironmentData",
            ),
            (
                "UpdatedEnvironmentData",
                "golem_common::model::environment::UpdatedEnvironmentData",
            ),
            (
                "Environment",
                "golem_common::model::environment::Environment",
            ),
            ("Component", "golem_common::model::component::Component"),
            ("AccountRole", "golem_common::model::auth::AccountRole"),
            (
                "EnvironmentShare",
                "golem_common::model::environment_share::EnvironmentShare",
            ),
            (
                "NewEnvironmentShareData",
                "golem_common::model::environment_share::NewEnvironmentShareData",
            ),
            (
                "UpdatedEnvironmentShareData",
                "golem_common::model::environment_share::UpdatedEnvironmentShareData",
            ),
            // ("DataSchema", "golem_common::model::agent::DataSchema"),
            // ("AgentInstanceKey", "golem_common::model::AgentInstanceKey"),
            ("AgentType", "golem_common::model::agent::AgentType"),
            ("AnalysedExport", "golem_wasm_ast::analysis::AnalysedExport"),
            ("AnalysedType", "golem_wasm_ast::analysis::AnalysedType"),
            // ("PluginScope", "golem_common::model::plugin::PluginScope"),
            (
                "ComponentMetadata",
                "golem_common::model::component_metadata::ComponentMetadata",
            ),
            // (
            //    "ComponentFilePathWithPermissionsList",
            //    "golem_common::model::ComponentFilePathWithPermissionsList",
            // ),
            ("ComponentType", "golem_common::model::ComponentType"),
            // ("Empty", "golem_common::model::Empty"),
            (
                "InitialComponentFile",
                "golem_common::model::InitialComponentFile",
            ),
            // ("ErrorBody", "golem_common::model::error::ErrorBody"),
            // ("ErrorsBody", "golem_common::model::error::ErrorsBody"),
            // ("GolemError", "golem_common::model::error::GolemError"),
            // (
            //     "PluginInstallationAction",
            //     "golem_common::model::plugin::PluginInstallationAction",
            // ),
            (
                "OplogCursor",
                "golem_common::model::public_oplog::OplogCursor",
            ),
            ("OplogRegion", "golem_common::model::regions::OplogRegion"),
            // (
            //     "ProjectActions",
            //     "golem_common::model::auth::ProjectActions",
            // ),
            // (
            //     "ProjectPermission",
            //     "golem_common::model::auth::ProjectPermission",
            // ),
            ("PromiseId", "golem_common::model::PromiseId"),
            (
                "PublicOplogEntry",
                "golem_common::model::public_oplog::PublicOplogEntry",
            ),
            ("ShardId", "golem_common::model::ShardId"),
            ("ValueAndType", "golem_wasm_rpc::ValueAndType"),
            (
                "ValueAndOptionalType",
                "golem_wasm_rpc::json::OptionallyValueAndTypeJson",
            ),
            (
                "WasiConfigVarsEntry",
                "golem_common::model::worker::WasiConfigVarsEntry",
            ),
            (
                "WasmRpcTarget",
                "golem_common::model::component_metadata::WasmRpcTarget",
            ),
            (
                "WorkerCreationRequest",
                "golem_common::model::worker::WorkerCreationRequest",
            ),
            ("WorkerFilter", "golem_common::model::WorkerFilter"),
            ("WorkerId", "golem_common::model::WorkerId"),
            (
                "WorkerBindingType",
                "golem_common::model::WorkerBindingType",
            ),
            ("WorkerStatus", "golem_common::model::WorkerStatus"),
        ],
        &["/v1/components/{component_id}/workers/{worker_name}/connect"],
    )
    .expect("Failed to generate client code from OpenAPI spec.");
}

fn copy_if_different(
    src: impl AsRef<Path> + Sized,
    dst: impl AsRef<Path> + Sized,
) -> std::io::Result<()> {
    if dst.as_ref().exists() {
        let a = std::fs::read(&src)?;
        let b = std::fs::read(&dst)?;
        if a != b {
            std::fs::copy(src, dst)?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst)?;
        Ok(())
    }
}

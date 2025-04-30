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

    println!("Output directory: {:?}", out_dir);
    println!("Workspace OpenAPI file: {:?}", root_yaml_path);

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
            ("AnalysedExport", "golem_wasm_ast::analysis::AnalysedExport"),
            ("AnalysedType", "golem_wasm_ast::analysis::AnalysedType"),
            (
                "ComponentMetadata",
                "golem_common::model::component_metadata::ComponentMetadata",
            ),
            (
                "ComponentFilePathWithPermissionsList",
                "golem_common::model::ComponentFilePathWithPermissionsList",
            ),
            ("ComponentType", "golem_common::model::ComponentType"),
            ("DefaultComponentOwner", "crate::DefaultComponentOwner"),
            (
                "DefaultPluginScope",
                "golem_common::model::plugin::DefaultPluginScope",
            ),
            ("DefaultPluginOwner", "crate::DefaultPluginOwner"),
            (
                "OplogCursor",
                "golem_common::model::public_oplog::OplogCursor",
            ),
            ("Empty", "golem_common::model::Empty"),
            (
                "InitialComponentFile",
                "golem_common::model::InitialComponentFile",
            ),
            ("ErrorBody", "golem_common::model::error::ErrorBody"),
            ("ErrorsBody", "golem_common::model::error::ErrorsBody"),
            ("GolemError", "golem_common::model::error::GolemError"),
            (
                "ValueAndOptionalType",
                "golem_wasm_rpc::json::OptionallyTypeAnnotatedValueJson",
            ),
            (
                "PluginInstallationAction",
                "golem_common::model::plugin::PluginInstallationAction",
            ),
            ("PromiseId", "golem_common::model::PromiseId"),
            ("ShardId", "golem_common::model::ShardId"),
            (
                "TypeAnnotatedValue",
                "golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue",
            ),
            (
                "WasmRpcTarget",
                "golem_common::model::component_metadata::WasmRpcTarget",
            ),
            ("WorkerFilter", "golem_common::model::WorkerFilter"),
            ("WorkerId", "golem_common::model::WorkerId"),
            (
                "WorkerBindingType",
                "golem_common::model::WorkerBindingType",
            ),
            ("WorkerStatus", "golem_common::model::WorkerStatus"),
            (
                "PublicOplogEntry",
                "golem_common::model::public_oplog::PublicOplogEntry",
            ),
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

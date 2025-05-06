use golem_openapi_client_generator::parse_openapi_specs;
use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn main() {
    println!("Starting code generation for Golem Cloud OpenAPI client.");
    let out_dir = var_os("OUT_DIR").unwrap();

    let root_yaml_path = PathBuf::from("../openapi/golem-cloud-service.yaml");
    let local_yaml_path = PathBuf::from("openapi/golem-cloud-service.yaml");

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

    println!("Generating code to {}", out_dir.to_string_lossy());
    generate(local_yaml_path.clone(), out_dir)
}

fn generate(yaml_path: PathBuf, out_dir: OsString) {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[yaml_path]).expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-cloud-client",
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
            ("ComponentType", "golem_common::model::ComponentType"),
            (
                "ComponentFilePathWithPermissionsList",
                "golem_common::model::ComponentFilePathWithPermissionsList",
            ),
            (
                "ComponentSearchParameters",
                "golem_client::model::ComponentSearchParameters",
            ),
            // (
            //     "ComponentEnv",
            //     "golem_client::model::ComponentEnv",
            // ),
            // (
            //     "CloudComponentOwner",
            //     "cloud_common::model::CloudComponentOwner",
            // ),
            ("CloudPluginScope", "crate::CloudPluginScope"),
            // ("CloudPluginOwner", "cloud_common::model::CloudPluginOwner"),
            (
                "GatewayBindingWithTypeInfo",
                "golem_client::model::GatewayBindingWithTypeInfo",
            ),
            (
                "HttpApiDefinitionResponseData",
                "golem_client::model::HttpApiDefinitionResponseData",
            ),
            ("Provider", "golem_client::model::Provider"),
            (
                "InitialComponentFile",
                "golem_common::model::InitialComponentFile",
            ),
            ("ErrorBody", "golem_client::model::ErrorBody"),
            ("ErrorsBody", "golem_client::model::ErrorsBody"),
            ("GolemError", "golem_client::model::GolemError"),
            ("MethodPattern", "golem_client::model::MethodPattern"),
            (
                "OplogCursor",
                "golem_common::model::public_oplog::OplogCursor",
            ),
            (
                "PluginInstallation",
                "golem_client::model::PluginInstallation",
            ),
            (
                "PluginInstallationCreation",
                "golem_client::model::PluginInstallationCreation",
            ),
            (
                "PluginTypeSpecificDefinition",
                "golem_client::model::PluginTypeSpecificDefinition",
            ),
            (
                "PluginInstallationUpdate",
                "golem_client::model::PluginInstallationUpdate",
            ),
            (
                "PluginTypeSpecificCreation",
                "golem_client::model::PluginTypeSpecificCreation",
            ),
            //("ProjectAction", "cloud_common::model::ProjectAction"),
            (
                "PluginInstallationAction",
                "golem_client::model::PluginInstallationAction",
            ),
            ("PromiseId", "golem_common::model::PromiseId"),
            ("RibInputTypeInfo", "golem_client::model::RibInputTypeInfo"),
            (
                "RouteWithTypeInfo",
                "golem_client::model::RouteWithTypeInfo",
            ),
            // ("Role", "cloud_common::model::Role"),
            ("ShardId", "golem_common::model::ShardId"),
            (
                "TypeAnnotatedValue",
                "golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue",
            ),
            ("WorkerFilter", "golem_common::model::WorkerFilter"),
            ("WorkerId", "golem_common::model::WorkerId"),
            (
                "WorkerServiceErrorsBody",
                "golem_client::model::WorkerServiceErrorsBody",
            ),
            ("WorkerStatus", "golem_common::model::WorkerStatus"),
            (
                "PublicOplogEntry",
                "golem_common::model::public_oplog::PublicOplogEntry",
            ),
            (
                "ValueAndOptionalType",
                "golem_wasm_rpc::json::OptionallyTypeAnnotatedValueJson",
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

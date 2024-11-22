use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use golem_openapi_client_generator::parse_openapi_specs;
use relative_path::RelativePath;

fn main() {
    println!("Starting code generation for Golem Cloud OpenAPI client.");
    let out_dir = var_os("OUT_DIR").unwrap();
    let manifest_dir = var_os("CARGO_MANIFEST_DIR").unwrap();

    let rel_path = RelativePath::new("../openapi/golem-cloud-service.yaml");
    let yaml_path = rel_path.to_logical_path(manifest_dir.clone());

    println!("Output directory: {:?}", out_dir);
    println!("Workspace OpenAPI file: {:?}", yaml_path);

    if yaml_path.exists() {
        generate(yaml_path.clone(), out_dir);

        // Copying the file to the crate so it gets packaged
        std::fs::create_dir_all(Path::new(&manifest_dir).join("openapi")).unwrap();
        std::fs::copy(
            yaml_path.clone(),
            Path::new(&manifest_dir).join("openapi/golem-cloud-service.yaml"),
        )
        .unwrap();

        println!("cargo::rerun-if-changed=build.rs");
        println!("cargo::rerun-if-changed={}", yaml_path.display());
    } else {
        let crate_yaml_path = Path::new(&manifest_dir).join("openapi/golem-cloud-service.yaml");
        generate(crate_yaml_path, out_dir);
    }
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
                "CloudComponentOwner",
                "cloud_common::model::CloudComponentOwner",
            ),
            ("CloudPluginScope", "crate::CloudPluginScope"),
            ("CloudPluginOwner", "cloud_common::model::CloudPluginOwner"),
            (
                "GatewayBindingWithTypeInfo",
                "golem_client::model::GatewayBindingWithTypeInfo",
            ),
            (
                "HttpApiDefinitionWithTypeInfo",
                "golem_client::model::HttpApiDefinitionWithTypeInfo",
            ),
            (
                "InitialComponentFile",
                "golem_common::model::InitialComponentFile",
            ),
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
            ("ProjectAction", "cloud_common::model::ProjectAction"),
            ("PromiseId", "golem_common::model::PromiseId"),
            ("RibInputTypeInfo", "golem_client::model::RibInputTypeInfo"),
            (
                "RouteWithTypeInfo",
                "golem_client::model::RouteWithTypeInfo",
            ),
            ("Role", "cloud_common::model::Role"),
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
        ],
    )
    .expect("Failed to generate client code from OpenAPI spec.");
}

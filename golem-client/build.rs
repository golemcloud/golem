use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use golem_openapi_client_generator::parse_openapi_specs;

fn main() {
    println!("Starting code generation for Golem OpenAPI client.");
    let out_dir = var_os("OUT_DIR").unwrap();
    let manifest_dir = var_os("CARGO_MANIFEST_DIR").unwrap();

    let yaml_path = Path::new(&manifest_dir).join("../openapi/golem-service.yaml");

    println!("Output directory: {:?}", out_dir);
    println!("Workspace OpenAPI file: {:?}", yaml_path);

    if yaml_path.exists() {
        generate(yaml_path.clone(), out_dir);

        // Copying the file to the crate so it gets packaged
        std::fs::create_dir_all(Path::new(&manifest_dir).join("openapi")).unwrap();
        std::fs::copy(
            yaml_path.clone(),
            Path::new(&manifest_dir).join("openapi/golem-service.yaml"),
        )
        .unwrap();

        println!("cargo::rerun-if-changed=build.rs");
        println!("cargo::rerun-if-changed={yaml_path:?}");
    } else {
        let crate_yaml_path = Path::new(&manifest_dir).join("openapi/golem-service.yaml");
        generate(crate_yaml_path, out_dir);
    }
}

fn generate(yaml_path: PathBuf, out_dir: OsString) {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[yaml_path.clone()]).expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-client",
        "0.0.0",
        false,
        true,
    )
    .expect("Failed to generate client code from OpenAPI spec.");
}

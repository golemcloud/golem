use std::env::var_os;
use std::path::Path;

use golem_openapi_client_generator::parse_openapi_specs;

fn main() {
    println!("Starting code generation for Golem OpenAPI client.");
    let out_dir = var_os("OUT_DIR").unwrap();

    println!("Output directory: {:?}", out_dir);

    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[Path::new("../openapi/golem-service.yaml").to_path_buf()])
            .expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-client",
        "0.0.0",
        false,
        true,
    )
    .expect("Failed to generate client code from OpenAPI spec.");

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../openapi/golem-service.yaml");
}

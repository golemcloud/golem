use golem_openapi_client_generator::parse_openapi_specs;
use std::env::var_os;
use std::path::Path;

fn main() {
    println!("Starting code generation for Golem Cloud OpenAPI client.");
    let out_dir = var_os("OUT_DIR").unwrap();

    println!("Output directory: {:?}", out_dir);

    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[Path::new("../openapi/cloud-spec.yaml").to_path_buf()])
            .expect("Failed to parse OpenAPI spec."),
        Path::new(&out_dir),
        "golem-cloud-client",
        "0.0.0",
        false,
        true,
    )
    .expect("Failed to generate client code from OpenAPI spec.");

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../openapi/cloud-spec.yaml");
}

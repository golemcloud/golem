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
        copy_if_different(
            yaml_path.clone(),
            Path::new(&manifest_dir).join("openapi/golem-cloud-service.yaml"),
        )
        .unwrap();

        println!("cargo::rerun-if-changed=build.rs");
        println!("cargo::rerun-if-changed=openapi/golem-cloud-service.yaml");
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

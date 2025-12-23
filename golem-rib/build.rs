use cargo_metadata::MetadataCommand;
use miette::miette;
use protox::prost::Message;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let golem_wasm_root = find_package_root("golem-wasm");

    let file_descriptors = protox::compile(
        [
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
        ],
        [&format!("{golem_wasm_root}/proto"), &"proto".to_string()],
    )?;

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let fd_path = out_dir.join("services.bin");

    std::fs::write(fd_path, file_descriptors.encode_to_vec())?;

    tonic_prost_build::configure()
        .build_server(true)
        .extern_path(".wasm.rpc", "::golem_wasm::protobuf")
        .include_file("mod.rs")
        .compile_fds(file_descriptors)
        .map_err(|e| miette!(e))?;

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

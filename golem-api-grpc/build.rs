use cargo_metadata::MetadataCommand;
use miette::miette;
use protox::prost::Message;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let golem_wasm_root = find_package_root("golem-wasm");
    let golem_rib_root = find_package_root("golem-rib");

    println!("cargo::rerun-if-changed={golem_wasm_root}/proto");
    println!("cargo::rerun-if-changed={golem_rib_root}/proto");
    println!("cargo::rerun-if-changed=proto");

    let file_descriptors = protox::compile(
        [
            "proto/golem/componentcompilation/v1/component_compilation_service.proto",
            "proto/golem/registry/v1/registry_service.proto",
            "proto/golem/shardmanager/v1/shard_manager_service.proto",
            "proto/golem/worker/v1/worker_service.proto",
            "proto/golem/workerexecutor/v1/worker_executor.proto",
            "proto/grpc/health/v1/health.proto",
        ],
        [
            &format!("{golem_wasm_root}/proto"),
            &format!("{golem_rib_root}/proto"),
            &"proto".to_string(),
        ],
    )?;

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let fd_path = out_dir.join("services.bin");

    std::fs::write(fd_path, file_descriptors.encode_to_vec())?;

    tonic_prost_build::configure()
        .build_server(true)
        .extern_path(".wasm.rpc", "::golem_wasm::protobuf")
        .extern_path(".golem.rib", "::rib::proto::golem::rib")
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

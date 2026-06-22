use miette::miette;
use protox::prost::Message;
use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        ["proto"],
    )?;

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let fd_path = out_dir.join("services.bin");

    std::fs::write(fd_path, file_descriptors.encode_to_vec())?;

    tonic_prost_build::configure()
        .build_server(true)
        .include_file("mod.rs")
        .compile_fds(file_descriptors)
        .map_err(|e| miette!(e))?;

    Ok(())
}

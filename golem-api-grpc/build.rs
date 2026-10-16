use heck::ToUpperCamelCase;
use miette::miette;
use protox::prost::Message;
use std::env;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema_proto_root =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../golem-schema/proto");

    println!("cargo::rerun-if-changed=proto");
    println!("cargo::rerun-if-changed={}", schema_proto_root.display());

    // The schema and tool protos are owned by golem-schema, which generates their Rust types.
    // Every type they define is mapped to golem-schema's generated code instead of being
    // generated again here.
    let mut schema_protos = Vec::new();
    collect_protos(&schema_proto_root, &schema_proto_root, &mut schema_protos)?;
    let schema_file_descriptors = protox::compile(&schema_protos, [&schema_proto_root])?;

    let file_descriptors = protox::compile(
        [
            "proto/golem/componentcompilation/v1/component_compilation_service.proto",
            "proto/golem/registry/v1/registry_service.proto",
            "proto/golem/shardmanager/v1/shard_manager_service.proto",
            "proto/golem/worker/v1/worker_service.proto",
            "proto/golem/workerexecutor/v1/worker_executor.proto",
            "proto/grpc/health/v1/health.proto",
        ],
        [Path::new("proto"), schema_proto_root.as_path()],
    )?;

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let fd_path = out_dir.join("services.bin");

    std::fs::write(fd_path, file_descriptors.encode_to_vec())?;

    let mut builder = tonic_prost_build::configure()
        .build_server(true)
        .include_file("mod.rs");

    for file in &schema_file_descriptors.file {
        let package = file.package();
        let rust_module = package.replace('.', "::");
        let type_names = file
            .message_type
            .iter()
            .map(|message| message.name())
            .chain(file.enum_type.iter().map(|enum_type| enum_type.name()));
        for type_name in type_names {
            builder = builder.extern_path(
                format!(".{package}.{type_name}"),
                format!(
                    "::golem_schema::proto::{rust_module}::{}",
                    type_name.to_upper_camel_case()
                ),
            );
        }
    }

    builder
        .compile_fds(file_descriptors)
        .map_err(|e| miette!(e))?;

    Ok(())
}

fn collect_protos(
    root: &Path,
    dir: &Path,
    protos: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_protos(root, &path, protos)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "proto")
        {
            protos.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

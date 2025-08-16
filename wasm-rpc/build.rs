use std::io::Result;

#[cfg(feature = "protobuf")]
fn main() -> Result<()> {
    use std::env;

    let wasm_ast_root =
        env::var("GOLEM_WASM_AST_ROOT").unwrap_or_else(|_| find_package_root("golem-wasm-ast"));

    let mut config = prost_build::Config::new();
    config.extern_path(".wasm.ast", "::golem_wasm_ast::analysis::protobuf");
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.type_attribute(
        ".",
        "#[cfg_attr(feature=\"bincode\", derive(bincode::Encode, bincode::Decode))]",
    );
    config.compile_protos(
        &[
            "proto/wasm/rpc/val.proto",
            "proto/wasm/rpc/witvalue.proto",
            "proto/wasm/rpc/value_and_type.proto",
        ],
        &[&format!("{wasm_ast_root}/proto"), &"proto".to_string()],
    )?;
    Ok(())
}

#[cfg(feature = "protobuf")]
fn find_package_root(name: &str) -> String {
    use cargo_metadata::MetadataCommand;

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

#[cfg(not(feature = "protobuf"))]
fn main() -> Result<()> {
    Ok(())
}

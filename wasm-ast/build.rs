#[cfg(feature = "protobuf")]
fn main() -> miette::Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.type_attribute(
        ".",
        "#[cfg_attr(feature=\"bincode\", derive(bincode::Encode, bincode::Decode))]",
    );

    let file_descriptors = protox::compile(["proto/wasm/ast/type.proto"], ["proto/"])?;
    config
        .compile_fds(file_descriptors)
        .map_err(|err| miette::miette!(err))?;
    Ok(())
}

#[cfg(not(feature = "protobuf"))]
fn main() -> Result<()> {
    Ok(())
}

#[cfg(feature = "host")]
fn main() -> miette::Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"host\")]");
    config.type_attribute(
        ".",
        "#[cfg_attr(feature=\"host\", derive(desert_rust::BinaryCodec))]",
    );

    let file_descriptors = protox::compile(
        [
            "proto/wasm/rpc/type.proto",
            "proto/wasm/rpc/val.proto",
            "proto/wasm/rpc/witvalue.proto",
            "proto/wasm/rpc/value_and_type.proto",
        ],
        [&"proto".to_string()],
    )?;

    config
        .compile_fds(file_descriptors)
        .map_err(|err| miette::miette!(err))?;
    Ok(())
}

#[cfg(not(feature = "host"))]
fn main() -> std::io::Result<()> {
    Ok(())
}

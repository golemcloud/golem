use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.type_attribute(
        ".",
        "#[cfg_attr(feature=\"bincode\", derive(bincode::Encode, bincode::Decode))]",
    );
    config.compile_protos(&["proto/wasm/ast/type.proto"], &["proto/"])?;
    Ok(())
}

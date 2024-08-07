use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.compile_protos(&["proto/wasm/ast/type.proto"], &["proto/"])?;
    Ok(())
}

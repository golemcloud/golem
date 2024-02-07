use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.compile_protos(
        &[
            "proto/type.proto",
            "proto/val.proto",
            "proto/witvalue.proto",
        ],
        &["proto/"],
    )?;
    Ok(())
}

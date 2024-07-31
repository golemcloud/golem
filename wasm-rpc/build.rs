use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();
    config.type_attribute(".", "#[cfg(feature = \"protobuf\")]");
    config.type_attribute(
        ".",
        "#[derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)]",
    );
    config.compile_protos(
        &[
            "proto/wasm/rpc/type.proto",
            "proto/wasm/rpc/val.proto",
            "proto/wasm/rpc/witvalue.proto",
            "proto/wasm/rpc/type_annotated_value.proto",
        ],
        &["proto/"],
    )?;
    Ok(())
}

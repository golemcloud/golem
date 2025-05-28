#[cfg(test)]
test_r::enable!();

#[allow(clippy::large_enum_variant)]
pub mod proto {
    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");
}

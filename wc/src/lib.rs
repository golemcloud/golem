mod grpc;
mod wit;
use prost_reflect::prost::Message;
use std::path::Path;
pub use wit::WitUtils;

pub fn from_grpc(protos_path: &[&Path], includes: &[&Path], version: Option<&str>) -> (wit::Wit, String, Vec<u8>) {
    let mut config = prost_build::Config::new();
    

    let file_descriptor_set = config.load_fds(protos_path, includes).unwrap();
    let wit = wit::Wit::from_fd(&file_descriptor_set, version);

    (
        wit,
        file_descriptor_set
            .file
            .last()
            .unwrap()
            .package()
            .to_string(),
        file_descriptor_set.encode_to_vec(),
    )
}

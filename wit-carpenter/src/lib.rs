mod grpc;
mod wit;
// mod openapi;
use prost::Message;
use std::path::Path;
pub use wit::WitUtils;

pub fn from_grpc(path: &Path, version: Option<&str>) -> (wit::Wit, String, Vec<u8>) {
    let mut config = prost_build::Config::new();

    let file_descriptor_set = config
        .load_fds(
            &[path.to_str().unwrap()],
            &[path.parent().expect("root dir").to_str().unwrap()],
        )
        .unwrap();
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

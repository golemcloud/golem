mod grpc;
mod wit;
use prost::Message;
use std::path::Path;
pub use wit::WitUtils;

pub fn from_grpc(path: &Path, version: Option<&str>) -> (wit::Wit, String, Vec<u8>) {
    let mut config = prost_build::Config::new();

    let (protos_root, proto_path) = if path.is_dir() {
        let path_string = path.to_str().unwrap().to_string();
        let default_path = "index.proto".to_string();
        (
            path_string.clone(),
            format!("{}/{}", path_string, default_path),
        )
    } else {
        (
            path.parent()
                .expect("Root dir for protos")
                .to_str()
                .unwrap()
                .to_string(),
            path.to_str().unwrap().to_string(),
        )
    };

    let file_descriptor_set = config.load_fds(&[proto_path], &[protos_root]).unwrap();

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

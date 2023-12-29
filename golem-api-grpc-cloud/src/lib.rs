pub mod proto {
    // use uuid::Uuid;
    tonic::include_proto!("mod");

    // tonic::include_proto!("../golem-api-grpc/proto/golem");
    // include!(concat!(env!("OUT_DIR"), concat!("/", "", ".rs")));

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");
}

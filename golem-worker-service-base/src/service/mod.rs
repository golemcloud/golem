pub mod api_definition;
pub mod api_definition_validator;
pub mod http_request_definition_lookup;
pub mod template;
pub mod worker;

pub fn with_metadata<T, I, K, V>(request: T, metadata: I) -> tonic::Request<T>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut req = tonic::Request::new(request);
    let req_metadata = req.metadata_mut();

    for (key, value) in metadata {
        let key = tonic::metadata::MetadataKey::from_bytes(key.as_ref().as_bytes());
        let value = value.as_ref().parse();
        if let (Ok(key), Ok(value)) = (key, value) {
            req_metadata.insert(key, value);
        }
    }

    req
}

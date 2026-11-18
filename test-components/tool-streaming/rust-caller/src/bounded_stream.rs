use std::future::Future;

pub async fn read_bounded<S, F, Fut>(mut stream: S, limit: usize, mut read: F) -> (Vec<u8>, bool)
where
    F: FnMut(S, Vec<u8>) -> Fut,
    Fut: Future<Output = (S, Vec<u8>)>,
{
    let mut bytes = Vec::with_capacity(limit);
    let at_eof = loop {
        let previous_len = bytes.len();
        let (returned_stream, returned) = read(stream, bytes).await;
        stream = returned_stream;
        bytes = returned;
        if bytes.len() == previous_len {
            break true;
        }
        if bytes.len() == limit {
            break false;
        }
    };
    (bytes, at_eof)
}

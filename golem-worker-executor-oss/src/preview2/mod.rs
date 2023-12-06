wasmtime::component::bindgen!({
    path: "../golem-wit/wit",
    interfaces: "
      import golem:api/host;

      import wasi:blobstore/blobstore;
      import wasi:blobstore/container;
      import wasi:blobstore/types;
      import wasi:keyvalue/atomic;
      import wasi:keyvalue/batch;
      import wasi:keyvalue/cache;
      import wasi:keyvalue/readwrite;
      import wasi:keyvalue/types;
      import wasi:keyvalue/wasi-cloud-error;
    ",
    tracing: false,
    async: true,
    with: {
        "wasi:io/streams/input-stream": InputStream,
        "wasi:io/streams/output-stream": OutputStream,
        "wasi:io/poll/pollable": Pollable,
        "wasi:blobstore/container/container": super::host::blobstore::types::ContainerEntry,
        "wasi:blobstore/container/stream-object-names": super::host::blobstore::types::StreamObjectNamesEntry,
        "wasi:blobstore/types/incoming-value": super::host::blobstore::types::IncomingValueEntry,
        "wasi:blobstore/types/outgoing-value": super::host::blobstore::types::OutgoingValueEntry,
        "wasi:keyvalue/wasi-cloud-error/error": super::host::keyvalue::error::ErrorEntry,
        "wasi:keyvalue/types/bucket": super::host::keyvalue::types::BucketEntry,
        "wasi:keyvalue/types/incoming-value": super::host::keyvalue::types::IncomingValueEntry,
        "wasi:keyvalue/types/outgoing-value": super::host::keyvalue::types::OutgoingValueEntry,
    }
});

pub type InputStream = wasmtime_wasi::preview2::InputStream;
pub type OutputStream = wasmtime_wasi::preview2::OutputStream;

pub type Pollable = wasmtime_wasi::preview2::Pollable;

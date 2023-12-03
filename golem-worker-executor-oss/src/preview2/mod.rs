wasmtime::component::bindgen!({
    path: "../golem-wit/wit",
    interfaces: "
      import golem:api/host

      import wasi:blobstore/blobstore
      import wasi:blobstore/container
      import wasi:blobstore/types
      import wasi:keyvalue/atomic
      import wasi:keyvalue/batch
      import wasi:keyvalue/cache
      import wasi:keyvalue/readwrite
      import wasi:keyvalue/types
      import wasi:keyvalue/wasi-cloud-error
    ",
    tracing: false,
    async: true,
    trappable_error_type: {
        "wasi:filesystem/types"::"error-code": Error,
    },
});

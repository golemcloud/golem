// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

wasmtime::component::bindgen!({
    path: "../golem-wit/wit",
    interfaces: "
      import golem:api/host;
      import golem:rpc/types@0.1.0;

      import wasi:blobstore/blobstore;
      import wasi:blobstore/container;
      import wasi:blobstore/types;
      import wasi:keyvalue/atomic@0.1.0;
      import wasi:keyvalue/eventual-batch@0.1.0;
      import wasi:keyvalue/cache@0.1.0;
      import wasi:keyvalue/eventual@0.1.0;
      import wasi:keyvalue/types@0.1.0;
      import wasi:keyvalue/wasi-keyvalue-error@0.1.0;
      import wasi:logging/logging;
    ",
    tracing: false,
    async: true,
    with: {
        "wasi:io/streams/input-stream": InputStream,
        "wasi:io/streams/output-stream": OutputStream,
        "wasi:io/poll/pollable": Pollable,
        "wasi:blobstore/container/container": super::durable_host::blobstore::types::ContainerEntry,
        "wasi:blobstore/container/stream-object-names": super::durable_host::blobstore::types::StreamObjectNamesEntry,
        "wasi:blobstore/types/incoming-value": super::durable_host::blobstore::types::IncomingValueEntry,
        "wasi:blobstore/types/outgoing-value": super::durable_host::blobstore::types::OutgoingValueEntry,
        "wasi:keyvalue/wasi-keyvalue-error/error": super::durable_host::keyvalue::error::ErrorEntry,
        "wasi:keyvalue/types/bucket": super::durable_host::keyvalue::types::BucketEntry,
        "wasi:keyvalue/types/incoming-value": super::durable_host::keyvalue::types::IncomingValueEntry,
        "wasi:keyvalue/types/outgoing-value": super::durable_host::keyvalue::types::OutgoingValueEntry,
    }
});

pub type InputStream = wasmtime_wasi::preview2::InputStream;
pub type OutputStream = wasmtime_wasi::preview2::OutputStream;

pub type Pollable = wasmtime_wasi::preview2::Pollable;

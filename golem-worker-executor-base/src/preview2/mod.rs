// Copyright 2024-2025 Golem Cloud
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

use golem_wasm_rpc::ValueAndType;
use std::mem;

wasmtime::component::bindgen!({
    path: r"../wit",
    world: "golem:api/golem",
    tracing: false,
    async: true,
    trappable_imports: true,
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
        "golem:api/host/get-workers": super::durable_host::golem::GetWorkersEntry,
        "golem:api/oplog/get-oplog": super::durable_host::golem::v1x::GetOplogEntry,
        "golem:api/oplog/search-oplog": super::durable_host::golem::v1x::SearchOplogEntry,
        "wasi:rdbms/mysql/db-connection": super::durable_host::rdbms::mysql::MysqlDbConnection,
        "wasi:rdbms/mysql/db-result-stream": super::durable_host::rdbms::mysql::DbResultStreamEntry,
        "wasi:rdbms/mysql/db-transaction": super::durable_host::rdbms::mysql::DbTransactionEntry,
        "wasi:rdbms/postgres/db-connection": super::durable_host::rdbms::postgres::PostgresDbConnection,
        "wasi:rdbms/postgres/db-result-stream": super::durable_host::rdbms::postgres::DbResultStreamEntry,
        "wasi:rdbms/postgres/db-transaction": super::durable_host::rdbms::postgres::DbTransactionEntry,
        "wasi:rdbms/postgres/lazy-db-column-type": super::durable_host::rdbms::postgres::LazyDbColumnTypeEntry,
        "wasi:rdbms/postgres/lazy-db-value": super::durable_host::rdbms::postgres::LazyDbValueEntry,
    },
});

pub type InputStream = wasmtime_wasi::InputStream;
pub type OutputStream = wasmtime_wasi::OutputStream;

pub type Pollable = wasmtime_wasi::Pollable;

impl From<golem_wasm_rpc::WitValue> for golem::rpc::types::WitValue {
    fn from(value: golem_wasm_rpc::WitValue) -> Self {
        unsafe { mem::transmute(value) }
    }
}

impl From<golem::rpc::types::WitValue> for golem_wasm_rpc::WitValue {
    fn from(value: golem::rpc::types::WitValue) -> Self {
        unsafe { mem::transmute(value) }
    }
}

impl From<golem_wasm_rpc::Value> for golem::rpc::types::WitValue {
    fn from(value: golem_wasm_rpc::Value) -> Self {
        let wit_value: golem_wasm_rpc::WitValue = value.into();
        wit_value.into()
    }
}

impl From<golem::rpc::types::WitValue> for golem_wasm_rpc::Value {
    fn from(value: golem::rpc::types::WitValue) -> Self {
        let wit_value: golem_wasm_rpc::WitValue = value.into();
        wit_value.into()
    }
}

impl From<ValueAndType> for golem::rpc::types::WitValue {
    fn from(value: ValueAndType) -> Self {
        let wit_value: golem_wasm_rpc::WitValue = value.into();
        wit_value.into()
    }
}

// reexports so that we don't have to change version numbers everywhere
pub use self::golem::api0_2_1 as golem_api_0_2_x;
pub use self::golem::api1_1_2 as golem_api_1_x;

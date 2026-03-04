// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

wasmtime::component::bindgen!({
    path: r"../wit",
    world: "golem:api/golem",
    imports: {
        default: async | trappable,
    },
    exports: { default: async },
    require_store_data_send: true,
    anyhow: true,
    with: {
        "wasi:io/streams.input-stream": InputStream,
        "wasi:io/streams.output-stream": OutputStream,
        "wasi:blobstore/container.container": super::durable_host::blobstore::types::ContainerEntry,
        "wasi:blobstore/container.stream-object-names": super::durable_host::blobstore::types::StreamObjectNamesEntry,
        "wasi:blobstore/types.incoming-value": super::durable_host::blobstore::types::IncomingValueEntry,
        "wasi:blobstore/types.outgoing-value": super::durable_host::blobstore::types::OutgoingValueEntry,
        "wasi:keyvalue/wasi-keyvalue-error.error": super::durable_host::keyvalue::error::ErrorEntry,
        "wasi:keyvalue/types.bucket": super::durable_host::keyvalue::types::BucketEntry,
        "wasi:keyvalue/types.incoming-value": super::durable_host::keyvalue::types::IncomingValueEntry,
        "wasi:keyvalue/types.outgoing-value": super::durable_host::keyvalue::types::OutgoingValueEntry,
        "golem:agent/common": golem_common::model::agent::bindings::golem::agent::common,
        "golem:api/context.span": super::durable_host::golem::invocation_context_api::SpanEntry,
        "golem:api/context.invocation-context": super::durable_host::golem::invocation_context_api::InvocationContextEntry,
        "golem:api/host.get-agents": super::durable_host::golem::v1x::GetAgentsEntry,
        "golem:api/host.get-promise-result": super::durable_host::golem::v1x::GetPromiseResultEntry,
        "golem:api/oplog.get-oplog": super::durable_host::golem::v1x::GetOplogEntry,
        "golem:api/oplog.search-oplog": super::durable_host::golem::v1x::SearchOplogEntry,
        "golem:durability/durability.lazy-initialized-pollable": super::durable_host::durability::LazyInitializedPollableEntry,
        "golem:core": golem_wasm::golem_core_1_5_x,
        "golem:agent/host.wasm-rpc": golem_wasm::WasmRpcEntry,
        "golem:agent/host.future-invoke-result": golem_wasm::FutureInvokeResultEntry,
        "golem:agent/host.cancellation-token": golem_wasm::CancellationTokenEntry,
        // shared wasi dependencies of golem:rpc/wasm-rpc and golem:api/golem
        "wasi:io/poll": wasmtime_wasi::p2::bindings::io::poll,
        "wasi:clocks/wall-clock": wasmtime_wasi::p2::bindings::clocks::wall_clock,
        "golem:rdbms/mysql.db-connection": super::durable_host::rdbms::mysql::MysqlDbConnection,
        "golem:rdbms/mysql.db-result-stream": super::durable_host::rdbms::mysql::DbResultStreamEntry,
        "golem:rdbms/mysql.db-transaction": super::durable_host::rdbms::mysql::DbTransactionEntry,
        "golem:rdbms/postgres.db-connection": super::durable_host::rdbms::postgres::PostgresDbConnection,
        "golem:rdbms/postgres.db-result-stream": super::durable_host::rdbms::postgres::DbResultStreamEntry,
        "golem:rdbms/postgres.db-transaction": super::durable_host::rdbms::postgres::DbTransactionEntry,
        "golem:rdbms/postgres.lazy-db-column-type": super::durable_host::rdbms::postgres::LazyDbColumnTypeEntry,
        "golem:rdbms/postgres.lazy-db-value": super::durable_host::rdbms::postgres::LazyDbValueEntry,
    },
});

pub type InputStream = wasmtime_wasi::DynInputStream;
pub type OutputStream = wasmtime_wasi::DynOutputStream;

pub type Pollable = wasmtime_wasi::p2::bindings::io::poll::Pollable;

// reexports so that we don't have to change version numbers everywhere
pub use self::golem::api1_5_0 as golem_api_1_x;
pub use self::golem::durability as golem_durability;
pub use golem_common::model::agent::bindings::golem::agent as golem_agent;

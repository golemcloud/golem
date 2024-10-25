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

use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::rdbms::RdbmsType;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::rdbms::types::{DbColumnTypeMeta, DbRow, DbValue, HostDbResultSet};
use crate::workerctx::WorkerCtx;

pub struct DbResultSetEntry {
    pub rdbms_type: RdbmsType,
    pub statement: String,
    pub params: Vec<DbValue>,
    pub columns: Vec<DbColumnTypeMeta>,
    pub rows: Option<Vec<DbRow>>,
}

impl DbResultSetEntry {
    pub fn new(rdbms_type: RdbmsType, statement: String, params: Vec<DbValue>, columns: Vec<DbColumnTypeMeta>, rows: Option<Vec<DbRow>>) -> Self {
        Self { rdbms_type, statement, params, columns, rows }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet  for &mut DurableWorkerCtx<Ctx> {
    async fn get_column_metadata(&mut self, self_: Resource<DbResultSetEntry>) -> anyhow::Result<Vec<DbColumnTypeMeta>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-column-metadata");
        let columns = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)
            .map(|e| {
                e.columns.clone()
            })?;

        Ok(columns)
    }

    async fn get_next(&mut self, self_: Resource<DbResultSetEntry>) -> anyhow::Result<Option<Vec<DbRow>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-next");
        let rows = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)
            .map(|e| {
                e.rows.clone()
            })?;

        Ok(rows)
    }

    fn drop(&mut self, rep: Resource<DbResultSetEntry>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::types::db-result-set", "drop");
        self.as_wasi_view().table().delete::<DbResultSetEntry>(rep)?;
        Ok(())
    }
}
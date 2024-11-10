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

use crate::durable_host::rdbms::types::DbResultSetEntry;
use crate::durable_host::rdbms::RdbmsType;
use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::rdbms::mysql::Host;
use crate::preview2::wasi::rdbms::mysql::HostDbConnection;
use crate::preview2::wasi::rdbms::types::{DbValue, Error};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}

pub struct MysqlDbConnection {
    pub address: String,
}

impl MysqlDbConnection {
    pub fn new(address: String) -> Self {
        Self { address }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<MysqlDbConnection>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "open");

        let worker_id = self.state.owned_worker_id.clone();
        let result = self
            .state
            .rdbms_service
            .mysql()
            .create(&worker_id, &address)
            .await;

        match result {
            Ok(_) => {
                let entry = MysqlDbConnection::new(address);
                let resource = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(resource))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "query");
        let worker_id = self.state.owned_worker_id.clone();
        let address = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&self_)?
            .address
            .clone();

        let result = self
            .state
            .rdbms_service
            .mysql()
            .query(
                &worker_id,
                &address,
                &statement,
                params.into_iter().map(|v| v.into()).collect(),
            )
            .await;

        match result {
            Ok(result) => {
                let entry = DbResultSetEntry::new(RdbmsType::Mysql, worker_id, result);
                let db_result_set = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(db_result_set))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn execute(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "execute");
        let worker_id = self.state.owned_worker_id.clone();
        let address = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&self_)?
            .address
            .clone();

        let result = self
            .state
            .rdbms_service
            .mysql()
            .execute(
                &worker_id,
                &address,
                &statement,
                params.into_iter().map(|v| v.into()).collect(),
            )
            .await
            .map_err(|e| e.into());

        Ok(result)
    }

    fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::mysql::db-connection", "drop");

        let worker_id = self.state.owned_worker_id.clone();
        let address = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&rep)?
            .address
            .clone();

        let _ = self
            .state
            .rdbms_service
            .mysql()
            .remove(&worker_id, &address);

        self.as_wasi_view()
            .table()
            .delete::<MysqlDbConnection>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for &mut DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<MysqlDbConnection>, Error>> {
        (*self).open(address).await
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        (*self).query(self_, statement, params).await
    }

    async fn execute(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        (*self).execute(self_, statement, params).await
    }

    fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        (*self).drop(rep)
    }
}

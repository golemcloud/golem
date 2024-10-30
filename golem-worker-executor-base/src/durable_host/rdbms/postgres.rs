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
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::rdbms::postgres::HostDbConnection;
use crate::preview2::wasi::rdbms::types::{DbResultSet, DbValue, Error};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

pub struct PostgresDbConnection {
    pub address: String,
}

impl PostgresDbConnection {
    pub fn new(address: String) -> Self {
        Self { address }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for &mut DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<PostgresDbConnection>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "open");

        let worker_id = self.state.owned_worker_id.clone();
        // let result = Durability::<Ctx, String, String, SerializableError>::wrap(
        //     self,
        //     WrappedFunctionType::ReadRemote,
        //     "golem rdbms::postgres::db-connection::open",
        //     address.clone(),
        //     |ctx| ctx.state.rdbms_service.postgres().create(address.clone().as_str()),
        // )
        //     .await;
        // match result {
        //     Ok(_) => {
        //         let entry = PostgresDbConnection::new(address);
        //         let resource = self.as_wasi_view().table().push(entry)?;
        //
        //
        //         Ok(Ok(resource))
        //     },
        //     Err(e) => Ok(Err(Error::Error(format!("{:?}", e)))),
        // }

        let _ = self
            .state
            .rdbms_service
            .postgres()
            .create(&worker_id, &address)
            .await
            .map_err(Error::Error)?;

        let entry = PostgresDbConnection::new(address);
        let resource = self.as_wasi_view().table().push(entry)?;

        Ok(Ok(resource))
    }

    async fn query(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSet>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "query");
        // let address = self
        //     .as_wasi_view()
        //     .table()
        //     .get::<PostgresDbConnection>(&self_)?
        //     .address
        //     .clone();
        // let result = Durability::<Ctx, (String, String), String, SerializableError>::wrap(
        //     self,
        //     WrappedFunctionType::ReadRemote,
        //     "golem rdbms::postgres::db-connection::query",
        //     (address.clone(), statement.clone()),
        //     |ctx| ctx.state.rdbms_service.query(address, statement),
        // )
        //     .await;
        // match result {
        //     Ok(_) => {
        //         let db_result_set = self
        //             .as_wasi_view()
        //             .table()
        //             .push(DbResultSetEntry::new(RdbmsType::Postgres, statement.clone(), params, vec![], vec![]))?;
        //         Ok(Ok(db_result_set))
        //     },
        //     Err(e) => Ok(Err(Error::Error(format!("{:?}", e)))),
        // }

        let db_result_set = self.as_wasi_view().table().push(DbResultSetEntry::new(
            RdbmsType::Postgres,
            statement.clone(),
            params,
            vec![],
            None,
        ))?;
        Ok(Ok(db_result_set))
    }

    async fn execute(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "execute");
        let worker_id = self.state.owned_worker_id.clone();
        let address = self
            .as_wasi_view()
            .table()
            .get::<PostgresDbConnection>(&self_)?
            .address
            .clone();

        let _ = self
            .state
            .rdbms_service
            .postgres()
            .execute(
                &worker_id,
                &address,
                &statement,
                params.into_iter().map(|v| v.into()).collect(),
            )
            .await
            .map_err(Error::Error)?;

        Ok(Ok(0))
    }

    fn drop(&mut self, rep: Resource<PostgresDbConnection>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::postgres::db-connection", "drop");

        // let worker_id = self.state.owned_worker_id.clone();
        // let address = self
        //     .as_wasi_view()
        //     .table()
        //     .get::<PostgresDbConnection>(&rep)?
        //     .address
        //     .clone();
        //
        // let _ = self.state
        //     .rdbms_service
        //     .postgres()
        //     .drop(&worker_id, &address).await.map_err(Error::Error)?;

        self.as_wasi_view()
            .table()
            .delete::<PostgresDbConnection>(rep)?;
        Ok(())
    }
}

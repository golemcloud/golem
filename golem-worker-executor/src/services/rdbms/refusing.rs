// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! The RDBMS service of a never-live owner. It refuses every operation that opens a
//! connection, begins, queries, resolves a transaction status or cleans up.

use super::*;

pub(crate) struct RefusingRdbmsService {
    pub(crate) inner: Arc<dyn RdbmsService>,
}

impl RdbmsService for RefusingRdbmsService {
    fn ignite(&self) -> Arc<dyn Rdbms<IgniteType>> {
        Arc::new(RefusingRdbms {
            inner: self.inner.ignite(),
        })
    }

    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>> {
        Arc::new(RefusingRdbms {
            inner: self.inner.mysql(),
        })
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>> {
        Arc::new(RefusingRdbms {
            inner: self.inner.postgres(),
        })
    }
}

struct RefusingRdbms<T: RdbmsType> {
    inner: Arc<dyn Rdbms<T>>,
}

#[async_trait]
impl<T: RdbmsType + 'static> Rdbms<T> for RefusingRdbms<T> {
    async fn create(
        &self,
        _address: &str,
        _agent_id: &AgentId,
    ) -> Result<RdbmsPoolKey, RdbmsError> {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::create").to_string(),
        ))
    }

    async fn exists(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.inner.exists(key, agent_id).await
    }

    async fn remove(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.inner.remove(key, agent_id).await
    }

    async fn execute(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
        _statement: &str,
        _params: Vec<T::DbValue>,
    ) -> Result<u64, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::execute").to_string(),
        ))
    }

    async fn query_stream(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
        _statement: &str,
        _params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::query_stream").to_string(),
        ))
    }

    async fn query(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
        _statement: &str,
        _params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::query").to_string(),
        ))
    }

    async fn begin_transaction(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, RdbmsError> {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::begin_transaction").to_string(),
        ))
    }

    async fn get_transaction_status(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
        _transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::get_transaction_status").to_string(),
        ))
    }

    async fn cleanup_transaction(
        &self,
        _key: &RdbmsPoolKey,
        _agent_id: &AgentId,
        _transaction_id: &TransactionId,
    ) -> Result<(), RdbmsError> {
        Err(RdbmsError::Other(
            crate::durable_host::never_live::refusal("rdbms::cleanup_transaction").to_string(),
        ))
    }

    async fn status(&self) -> RdbmsStatus {
        self.inner.status().await
    }
}

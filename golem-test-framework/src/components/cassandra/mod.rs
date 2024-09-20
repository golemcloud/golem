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

use scylla::{transport::session::PoolSize, Session, SessionBuilder};
use std::{num::NonZeroUsize, sync::Arc};
use tonic::async_trait;

pub mod docker;

#[async_trait]
pub trait Cassandra {
    fn assert_valid(&self);

    fn private_known_nodes(&self) -> Vec<String>;

    fn public_known_nodes(&self) -> Vec<String>;

    fn kill(&self);

    async fn try_get_session(&self, keyspace: Option<&str>) -> Result<Arc<Session>, String> {
        let mut session_builder = SessionBuilder::new()
            .known_nodes(self.public_known_nodes())
            .pool_size(PoolSize::PerHost(NonZeroUsize::new(10).unwrap()));

        if let Some(keyspace) = keyspace {
            session_builder = session_builder.use_keyspace(keyspace, false)
        };

        let session = session_builder
            .build()
            .await
            .map_err(|e| e.to_string())
            .unwrap();

        Ok(Arc::new(session))
    }

    async fn get_session(&self, keyspace: Option<&str>) -> Arc<Session> {
        self.assert_valid();
        self.try_get_session(keyspace).await.unwrap()
    }

    async fn flush_keyspace(&self, keyspace: &str) {
        let session = self.get_session(Some(keyspace)).await;
        session
            .query_unpaged(format!("TRUNCATE  {}.{};", keyspace, "kv_store"), &[])
            .await
            .unwrap();
        session
            .query_unpaged(format!("TRUNCATE  {}.{};", keyspace, "kv_sets"), &[])
            .await
            .unwrap();
        session
            .query_unpaged(format!("TRUNCATE  {}.{};", keyspace, "kv_sorted_sets"), &[])
            .await
            .unwrap();
    }
}

const DEFAULT_PORT: u16 = 9042;

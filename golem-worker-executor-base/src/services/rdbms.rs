use crate::services::rdbms::postgres::{Postgres, PostgresDefault};
use std::error::Error;
use std::sync::Arc;

pub trait RdbmsService {
    fn postgres(&self) -> Arc<dyn Postgres + Send + Sync>;
}

#[derive(Clone)]
pub struct RdbmsServiceDefault {
    postgres: Arc<dyn Postgres + Send + Sync>,
}

impl RdbmsServiceDefault {
    pub fn new(postgres: Arc<dyn Postgres + Send + Sync>) -> Self {
        Self { postgres }
    }
}

impl Default for RdbmsServiceDefault {
    fn default() -> Self {
        Self::new(Arc::new(PostgresDefault::default()))
    }
}

impl RdbmsService for RdbmsServiceDefault {
    fn postgres(&self) -> Arc<dyn Postgres + Send + Sync> {
        self.postgres.clone()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RdbmsPoolConfig {
    pub max_connections: u32,
}

impl Default for RdbmsPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RdbmsPoolKey {
    pub address: String,
}

impl RdbmsPoolKey {
    pub fn new(address: String) -> Self {
        Self { address }
    }
}

pub mod postgres {
    use crate::services::rdbms::types::{DbValue, DbValuePrimitive};
    use crate::services::rdbms::{RdbmsPoolConfig, RdbmsPoolKey};
    use async_trait::async_trait;
    use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
    use golem_common::model::OwnedWorkerId;
    use sqlx::postgres::{PgArguments, PgPoolOptions};
    use sqlx::Pool;
    use std::collections::HashSet;
    use std::ops::Deref;
    use chrono::DateTime;
    use sqlx::query::Query;
    use tracing::info;
    use uuid::Uuid;

    #[async_trait]
    pub trait Postgres {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String>;
        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool;
        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String>;
        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, String>;
    }

    #[derive(Clone)]
    pub struct PostgresDefault {
        pool_config: RdbmsPoolConfig,
        pool_cache: Cache<RdbmsPoolKey, (), Pool<sqlx::Postgres>, String>,
    }

    impl PostgresDefault {
        pub fn new(pool_config: RdbmsPoolConfig) -> Self {
            let pool_cache = Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "rdbms-postgres-pools",
            );
            Self {
                pool_config,
                pool_cache,
            }
        }
    }

    impl Default for PostgresDefault {
        fn default() -> Self {
            Self::new(RdbmsPoolConfig::default())
        }
    }

    #[async_trait]
    impl Postgres for PostgresDefault {
        async fn create(&self, _worker_id: &OwnedWorkerId, address: &str) -> Result<(), String> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool_config = self.pool_config.clone();
            let _pool = self
                .pool_cache
                .get_or_insert_simple(&key.clone(), || {
                    Box::pin(async move { create_pool(&key, &pool_config).await })
                })
                .await?;

            Ok(())
        }

        async fn remove(&self, _worker_id: &OwnedWorkerId, address: &str) -> Result<(), String> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool = self.pool_cache.get(&key).await;
            if let Some(pool) = pool {
                self.pool_cache.remove(&key);
                pool.close().await;
            }
            Ok(())
        }

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool = self.pool_cache.get(&key).await;
            pool.is_some()
        }

        async fn execute(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, String> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool = self.pool_cache.get(&key).await;
            if let Some(pool) = pool {
                let mut query: Query<sqlx::Postgres, PgArguments> = sqlx::query(statement);

                for param in params {
                    query = bind_value(query, param)?;
                }

                let result = query
                    .execute(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(result.rows_affected())
            } else {
                Err("DB Connection not found".to_string())
            }
        }
    }


    fn bind_value(query: Query<sqlx::Postgres, PgArguments>, value: DbValue) -> Result<Query<sqlx::Postgres, PgArguments>, String> {
        match value {
            DbValue::Primitive(v) => bind_value_primitive(query, v),
            DbValue::Array(v) => Err("Array param not supported".to_string()),
        }
    }


    fn bind_value_primitive(query: Query<sqlx::Postgres, PgArguments>, value: DbValuePrimitive) -> Result<Query<sqlx::Postgres, PgArguments>, String> {
       match value {
           DbValuePrimitive::Integer(v) => Ok(query.bind(v)),
           DbValuePrimitive::Decimal(v) => Ok(query.bind(v)),
           DbValuePrimitive::Float(v) => Ok(query.bind(v)),
           DbValuePrimitive::Boolean(v) => Ok(query.bind(v)),
           DbValuePrimitive::Chars(v) => Ok(query.bind(v)),
           DbValuePrimitive::Text(v) => Ok(query.bind(v)),
           DbValuePrimitive::Binary(v) => Ok(query.bind(v)),
           DbValuePrimitive::Blob(v) => Ok(query.bind(v)),
           DbValuePrimitive::Uuid(v) => Ok(query.bind(v)),
           DbValuePrimitive::Json(v) => Ok(query.bind(v)),
           DbValuePrimitive::Xml(v) => Ok(query.bind(v)),
           DbValuePrimitive::Spatial(v) => Ok(query.bind(v)),
           DbValuePrimitive::Enumeration(v) => Ok(query.bind(v)),
           DbValuePrimitive::Other(_, v) => Ok(query.bind(v)),
           DbValuePrimitive::Datetime(v) => Ok(query.bind(chrono::DateTime::from_timestamp_millis(v as i64))),
           DbValuePrimitive::Interval(v) => Ok(query.bind(chrono::Duration::milliseconds(v as i64 ))),
           DbValuePrimitive::DbNull => Ok(query.bind(None::<String>)),
       }
    }

    async fn create_pool(
        key: &RdbmsPoolKey,
        pool_config: &RdbmsPoolConfig,
    ) -> Result<Pool<sqlx::Postgres>, String> {
        info!(
            "DB Pool: {}, connections: {}",
            key.address, pool_config.max_connections
        );

        PgPoolOptions::new()
            .max_connections(pool_config.max_connections)
            .connect(&key.address)
            .await
            .map_err(|e| e.to_string())
    }
}

pub mod types {
    use async_trait::async_trait;
    use std::collections::HashSet;
    use uuid::Uuid;

    #[async_trait]
    pub trait DbResultSet {
        async fn get_column_metadata(&mut self) -> Result<Vec<DbColumnTypeMeta>, String>;

        async fn get_next(&mut self) -> Result<Option<Vec<DbRow>>, String>;
    }

    #[derive(Clone, Debug)]
    pub enum DbColumnTypePrimitive {
        Integer(Option<u8>),
        Decimal(u8, u8),
        Float,
        Boolean,
        Datetime,
        Interval,
        Chars(Option<u32>),
        Text,
        Binary(Option<u32>),
        Blob,
        Enumeration(Vec<String>),
        Json,
        Xml,
        Uuid,
        Spatial,
    }

    #[derive(Clone, Debug)]
    pub enum DbColumnType {
        Primitive(DbColumnTypePrimitive),
        Array(Vec<Option<u32>>, DbColumnTypePrimitive),
    }

    #[derive(Clone, Debug)]
    pub enum DbValuePrimitive {
        Integer(i64),
        Decimal(String),
        Float(f64),
        Boolean(bool),
        Datetime(u64),
        Interval(u64),
        Chars(String),
        Text(String),
        Binary(Vec<u8>),
        Blob(Vec<u8>),
        Enumeration(String),
        Json(String),
        Xml(String),
        Uuid(Uuid),
        Spatial(Vec<f64>),
        Other(String, Vec<u8>),
        DbNull,
    }

    #[derive(Clone, Debug)]
    pub enum DbValue {
        Primitive(DbValuePrimitive),
        Array(Vec<DbValuePrimitive>),
    }

    #[derive(Clone, Debug)]
    pub struct DbRow {
        pub values: Vec<DbValue>,
    }

    #[derive(Clone, Debug)]
    pub struct DbColumnTypeMeta {
        pub name: String,
        pub db_type: DbColumnType,
        pub db_type_flags: HashSet<DbColumnTypeFlag>,
        pub foreign_key: Option<String>,
    }

    #[derive(Clone, Debug)]
    pub enum DbColumnTypeFlag {
        PrimaryKey,
        ForeignKey,
        Unique,
        Nullable,
        Generated,
        AutoIncrement,
        DefaultValue,
        Indexed,
    }
}

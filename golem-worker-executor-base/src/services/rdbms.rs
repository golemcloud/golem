use crate::services::rdbms::postgres::{Postgres, PostgresDefault, PostgresNoOp};
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
    use crate::services::rdbms::types::{
        DbColumnType, DbColumnTypeMeta, DbColumnTypePrimitive, DbResultSet, DbValue,
        DbValuePrimitive, SimpleDbResultSet,
    };
    use crate::services::rdbms::{RdbmsPoolConfig, RdbmsPoolKey};
    use async_trait::async_trait;
    use chrono::DateTime;
    use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
    use golem_common::model::OwnedWorkerId;
    use sqlx::postgres::{PgArguments, PgColumn, PgPoolOptions, PgTypeInfo, PgTypeKind};
    use sqlx::query::Query;
    use sqlx::{Column, Execute, Pool, Row};
    use std::collections::HashSet;
    use std::ops::Deref;
    use std::sync::Arc;
    use tracing::{error, info};
    use uuid::Uuid;

    #[async_trait]
    pub trait Postgres {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String>;

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool;

        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<bool, String>;

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, String>;

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, String>;
    }

    #[derive(Clone, Default)]
    pub struct PostgresNoOp {}

    #[async_trait]
    impl Postgres for PostgresNoOp {
        async fn create(&self, _worker_id: &OwnedWorkerId, address: &str) -> Result<(), String> {
            info!("create connection - address: {}", address);
            Ok(())
        }

        async fn exists(&self, _worker_id: &OwnedWorkerId, _address: &str) -> bool {
            false
        }

        async fn remove(&self, _worker_id: &OwnedWorkerId, _address: &str) -> Result<bool, String> {
            Ok(false)
        }

        async fn execute(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            _params: Vec<DbValue>,
        ) -> Result<u64, String> {
            info!("execute - address: {}, statement: {}", address, statement);
            Ok(0)
        }

        async fn query(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            _params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, String> {
            info!("query - address: {}, statement: {}", address, statement);
            Ok(Arc::new(SimpleDbResultSet::empty()))
        }
    }

    #[derive(Clone)]
    pub struct PostgresDefault {
        pool_config: RdbmsPoolConfig,
        pool_cache: Cache<RdbmsPoolKey, (), Arc<Pool<sqlx::Postgres>>, String>,
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

        async fn get_or_create(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
        ) -> Result<Arc<Pool<sqlx::Postgres>>, String> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool_config = self.pool_config.clone();

            self.pool_cache
                .get_or_insert_simple(&key.clone(), || {
                    Box::pin(async move { Ok(Arc::new(create_pool(&key, &pool_config).await?)) })
                })
                .await
        }
    }

    impl Default for PostgresDefault {
        fn default() -> Self {
            Self::new(RdbmsPoolConfig::default())
        }
    }

    #[async_trait]
    impl Postgres for PostgresDefault {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String> {
            info!("create connection - address: {}", address);
            let _pool = self.get_or_create(worker_id, address).await?;
            info!("create connection - address: {} - done", address);
            Ok(())
        }

        async fn remove(&self, _worker_id: &OwnedWorkerId, address: &str) -> Result<bool, String> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool = self.pool_cache.try_get(&key);
            if let Some(pool) = pool {
                self.pool_cache.remove(&key);
                pool.close().await;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool {
            let key = RdbmsPoolKey::new(address.to_string());
            self.pool_cache.contains_key(&key)
        }

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, String> {
            info!(
                "execute - address: {}, statement: {} - 0",
                address, statement
            );
            let pool = self.get_or_create(worker_id, address).await?;
            info!(
                "execute - address: {}, statement: {} - 1",
                address, statement
            );
            let result = execute(statement, params, pool.deref()).await;
            info!(
                "execute - address: {}, statement: {} - 2",
                address, statement
            );
            result
        }

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, String> {
            info!("query - address: {}, statement: {} - 0", address, statement);
            let pool = self.get_or_create(worker_id, address).await?;
            info!("query - address: {}, statement: {} - 1", address, statement);
            let result = query(statement, params, pool.deref()).await;
            info!("query - address: {}, statement: {} - 2", address, statement);
            result
        }
    }

    async fn query(
        statement: &str,
        params: Vec<DbValue>,
        pool: &Pool<sqlx::Postgres>,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, String> {
        let mut query: Query<sqlx::Postgres, PgArguments> = sqlx::query(statement);

        for param in params {
            query = bind_value(query, param)?;
        }

        let result = query.fetch_all(pool).await.map_err(|e| e.to_string())?;

        if result.is_empty() {
            Ok(Arc::new(SimpleDbResultSet::empty()))
        } else {
            let first = &result[0];
            let _columns = first.columns();
            let columns = vec![];
            let values = vec![];
            Ok(Arc::new(SimpleDbResultSet::new(columns, Some(values))))
        }
    }

    async fn execute(
        statement: &str,
        params: Vec<DbValue>,
        pool: &Pool<sqlx::Postgres>,
    ) -> Result<u64, String> {
        let mut query: Query<sqlx::Postgres, PgArguments> = sqlx::query(statement);

        for param in params {
            query = bind_value(query, param)?;
        }

        let result = query.execute(pool).await.map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    // fn get_column_types(columns: &[PgColumn]) -> Result<Vec<DbColumnTypeMeta>, String> {
    //     let mut result = vec![columns.len()];
    //
    //     for column in columns {
    //         result.push(DbColumnTypeMeta {
    //             name: column.name().to_string(),
    //             db_type: column.type_info(),
    //             nullable: column.is_nullable(),
    //         });
    //     }
    //     Ok(result)
    // }
    //
    //
    // impl TryFrom<PgTypeInfo> for DbColumnType {
    //     type Error = String;
    //
    //     fn try_from(value: PgTypeInfo) -> Result<Self, Self::Error> {
    //
    //         let kind = value.kind();
    //
    //     }
    // }

    fn bind_value(
        query: Query<sqlx::Postgres, PgArguments>,
        value: DbValue,
    ) -> Result<Query<sqlx::Postgres, PgArguments>, String> {
        match value {
            DbValue::Primitive(v) => bind_value_primitive(query, v),
            DbValue::Array(_) => Err("Array param not supported".to_string()),
        }
    }

    fn bind_value_primitive(
        query: Query<sqlx::Postgres, PgArguments>,
        value: DbValuePrimitive,
    ) -> Result<Query<sqlx::Postgres, PgArguments>, String> {
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
            DbValuePrimitive::Datetime(v) => {
                Ok(query.bind(chrono::DateTime::from_timestamp_millis(v as i64)))
            }
            DbValuePrimitive::Interval(v) => {
                Ok(query.bind(chrono::Duration::milliseconds(v as i64)))
            }
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

        let pool = PgPoolOptions::new()
            .max_connections(pool_config.max_connections)
            .connect(&key.address)
            .await
            .map_err(|e| {
                error!(
                    "DB Pool: {}, connections: {} -  error {}",
                    key.address, pool_config.max_connections, e
                );
                e.to_string()
            })?;
        info!(
            "DB Pool: {}, connections: {} - created",
            key.address, pool_config.max_connections
        );
        Ok(pool)
    }
}

pub mod types {
    use async_trait::async_trait;
    use golem_common::tracing::directive::default::info;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use tracing::{error, info};
    use uuid::Uuid;

    #[async_trait]
    pub trait DbResultSet {
        async fn get_column_metadata(&self) -> Result<Vec<DbColumnTypeMeta>, String>;

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, String>;
    }

    #[derive(Clone, Debug)]
    pub struct SimpleDbResultSet {
        column_metadata: Vec<DbColumnTypeMeta>,
        rows: Arc<Mutex<Option<Vec<DbRow>>>>,
    }

    impl SimpleDbResultSet {
        pub fn new(column_metadata: Vec<DbColumnTypeMeta>, rows: Option<Vec<DbRow>>) -> Self {
            Self {
                column_metadata,
                rows: Arc::new(Mutex::new(rows)),
            }
        }

        pub fn empty() -> Self {
            Self::new(vec![], None)
        }
    }

    #[async_trait]
    impl DbResultSet for SimpleDbResultSet {
        async fn get_column_metadata(&self) -> Result<Vec<DbColumnTypeMeta>, String> {
            info!("get_column_metadata");
            Ok(self.column_metadata.clone())
        }

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, String> {
            let rows = self.rows.lock().unwrap().clone();
            info!("get_next {}", rows.is_some());
            if rows.is_some() {
                *self.rows.lock().unwrap() = None;
            }
            Ok(rows)
        }
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

#[cfg(test)]
mod tests {
    use crate::services::rdbms::types::{DbValue, DbValuePrimitive};
    use crate::services::rdbms::RdbmsService;
    use crate::services::rdbms::{RdbmsPoolKey, RdbmsServiceDefault};
    use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, WorkerId};
    use std::hash::Hash;
    use test_r::{test, timeout};
    use uuid::Uuid;

    #[test]
    #[timeout(30000)]
    async fn test() {
        let rdbms_service = RdbmsServiceDefault::default();

        let address = "postgresql://postgres:postgres@localhost:5444/postgres";

        let worker_id = OwnedWorkerId::new(
            &AccountId::generate(),
            &WorkerId {
                component_id: ComponentId::new_v4(),
                worker_name: "test".to_string(),
            },
        );

        let connection = rdbms_service.postgres().create(&worker_id, &address).await;

        assert!(connection.is_ok());

        let result = rdbms_service
            .postgres()
            .execute(&worker_id, &address, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());

        let connection = rdbms_service.postgres().create(&worker_id, &address).await;

        assert!(connection.is_ok());

        let exists = rdbms_service.postgres().exists(&worker_id, &address).await;

        assert!(exists);

        let result = rdbms_service
            .postgres()
            .query(&worker_id, &address, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());

        let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS components
            (
                component_id        uuid    NOT NULL PRIMARY KEY,
                namespace           text    NOT NULL,
                name                text    NOT NULL
            );
        "#;

        let insert_statement = r#"
            INSERT INTO components
            (component_id, namespace, name)
            VALUES
            ($1, $2, $3)
        "#;

        let result = rdbms_service
            .postgres()
            .execute(&worker_id, &address, create_table_statement, vec![])
            .await;

        assert!(result.is_ok());

        let params: Vec<DbValue> = vec![
            DbValue::Primitive(DbValuePrimitive::Uuid(Uuid::new_v4())),
            DbValue::Primitive(DbValuePrimitive::Text("default".to_string())),
            DbValue::Primitive(DbValuePrimitive::Text(format!("name-{}", Uuid::new_v4()))),
        ];

        let result = rdbms_service
            .postgres()
            .execute(&worker_id, &address, insert_statement, params)
            .await;

        assert!(result.is_ok_and(|v| v == 1));

        let result = rdbms_service.postgres().remove(&worker_id, &address).await;

        assert!(result.is_ok_and(|v| v));

        let exists = rdbms_service.postgres().exists(&worker_id, &address).await;

        assert!(!exists);
    }
}

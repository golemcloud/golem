use crate::services::rdbms::mysql::{Mysql, MysqlDefault};
use crate::services::rdbms::postgres::{Postgres, PostgresDefault};
use std::sync::Arc;

pub trait RdbmsService {
    fn mysql(&self) -> Arc<dyn Mysql + Send + Sync>;
    fn postgres(&self) -> Arc<dyn Postgres + Send + Sync>;
}

#[derive(Clone)]
pub struct RdbmsServiceDefault {
    mysql: Arc<dyn Mysql + Send + Sync>,
    postgres: Arc<dyn Postgres + Send + Sync>,
}

impl RdbmsServiceDefault {
    pub fn new(
        mysql: Arc<dyn Mysql + Send + Sync>,
        postgres: Arc<dyn Postgres + Send + Sync>,
    ) -> Self {
        Self { mysql, postgres }
    }
}

impl Default for RdbmsServiceDefault {
    fn default() -> Self {
        Self::new(
            Arc::new(MysqlDefault::default()),
            Arc::new(PostgresDefault::default()),
        )
    }
}

impl RdbmsService for RdbmsServiceDefault {
    fn mysql(&self) -> Arc<dyn Mysql + Send + Sync> {
        self.mysql.clone()
    }

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

pub mod sqlx_common {
    use crate::services::rdbms::types::{DbColumn, DbResultSet, DbRow, DbValue, Error};
    use crate::services::rdbms::{RdbmsPoolConfig, RdbmsPoolKey};
    use async_trait::async_trait;
    use futures_util::stream::BoxStream;
    use futures_util::StreamExt;
    use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
    use golem_common::model::OwnedWorkerId;
    use sqlx::database::HasArguments;
    use sqlx::{Database, Pool, Row};
    use std::ops::Deref;
    use std::sync::Arc;
    use tracing::{error, info};

    #[derive(Clone)]
    pub(crate) struct SqlxRdbms<DB>
    where
        DB: Database,
    {
        name: &'static str,
        pool_config: RdbmsPoolConfig,
        pool_cache: Cache<RdbmsPoolKey, (), Arc<Pool<DB>>, Error>,
    }

    impl<DB> SqlxRdbms<DB>
    where
        DB: Database,
        Pool<DB>: QueryExecutor,
        RdbmsPoolKey: PoolCreator<DB>,
    {
        pub(crate) fn new(name: &'static str, pool_config: RdbmsPoolConfig) -> Self {
            let cache_name: &'static str = format!("rdbms-{}-pools", name).leak();
            let pool_cache = Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                cache_name,
            );
            Self {
                name,
                pool_config,
                pool_cache,
            }
        }

        async fn get_or_create(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
        ) -> Result<Arc<Pool<DB>>, Error> {
            let key = RdbmsPoolKey::new(address.to_string());
            let pool_config = self.pool_config.clone();
            let name = self.name.to_string();
            self.pool_cache
                .get_or_insert_simple(&key.clone(), || {
                    Box::pin(async move {
                        info!(
                            "{} DB Pool: {}, connections: {}",
                            name, key.address, pool_config.max_connections
                        );
                        let result = key.create_pool(&pool_config).await.map_err(|e| {
                            error!(
                                "{} DB Pool: {}, connections: {} - error {}",
                                name, key.address, pool_config.max_connections, e
                            );
                            Error::ConnectionFailure(e.to_string())
                        })?;
                        Ok(Arc::new(result))
                    })
                })
                .await
        }

        pub(crate) async fn create(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
        ) -> Result<(), Error> {
            info!("{} create connection - address: {}", self.name, address);
            let _pool = self.get_or_create(worker_id, address).await?;
            Ok(())
        }

        pub(crate) async fn remove(
            &self,
            _worker_id: &OwnedWorkerId,
            address: &str,
        ) -> Result<bool, Error> {
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

        pub(crate) async fn exists(&self, _worker_id: &OwnedWorkerId, address: &str) -> bool {
            let key = RdbmsPoolKey::new(address.to_string());
            self.pool_cache.contains_key(&key)
        }

        pub(crate) async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, Error> {
            info!(
                "{} execute - address: {}, statement: {}",
                self.name, address, statement
            );

            let result = {
                let pool = self.get_or_create(worker_id, address).await?;
                pool.deref().execute(statement, params).await
            };

            result.map_err(|e| {
                error!(
                    "{} execute - address: {}, statement: {} - error: {}",
                    self.name, address, statement, e
                );
                e
            })
        }

        pub(crate) async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
            info!(
                "{} query - address: {}, statement: {}",
                self.name, address, statement
            );

            let result = {
                let pool = self.get_or_create(worker_id, address).await?;
                pool.deref().query_stream(statement, params, 50).await
            };

            result.map_err(|e| {
                error!(
                    "{} query - address: {}, statement: {} - error: {}",
                    self.name, address, statement, e
                );
                e
            })
        }
    }

    #[async_trait]
    pub(crate) trait PoolCreator<DB: Database> {
        async fn create_pool(&self, config: &RdbmsPoolConfig) -> Result<Pool<DB>, sqlx::Error>;
    }

    #[async_trait]
    pub(crate) trait QueryExecutor {
        async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error>;

        async fn query_stream(
            &self,
            statement: &str,
            params: Vec<DbValue>,
            batch: usize,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;
    }

    #[derive(Clone)]
    pub struct StreamDbResultSet<'q, DB: Database> {
        columns: Vec<DbColumn>,
        first_rows: Arc<async_mutex::Mutex<Option<Vec<DbRow>>>>,
        row_stream: Arc<async_mutex::Mutex<BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>>>,
    }

    impl<'q, DB: Database> StreamDbResultSet<'q, DB>
    where
        DB::Row: Row,
        DbRow: for<'a> TryFrom<&'a DB::Row, Error = String>,
        DbColumn: for<'a> TryFrom<&'a DB::Column, Error = String>,
    {
        fn new(
            columns: Vec<DbColumn>,
            first_rows: Vec<DbRow>,
            row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>,
        ) -> Self {
            Self {
                columns,
                first_rows: Arc::new(async_mutex::Mutex::new(Some(first_rows))),
                row_stream: Arc::new(async_mutex::Mutex::new(row_stream)),
            }
        }

        pub(crate) async fn create(
            stream: BoxStream<'q, Result<DB::Row, sqlx::Error>>,
            batch: usize,
        ) -> Result<StreamDbResultSet<'q, DB>, Error> {
            let mut row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>> =
                Box::pin(stream.chunks(batch));

            let first: Option<Vec<Result<DB::Row, sqlx::Error>>> = row_stream.next().await;

            match first {
                Some(rows) if !rows.is_empty() => {
                    let rows: Vec<DB::Row> = rows
                        .into_iter()
                        .map(|r| r.map_err(|e| e.to_string()))
                        .collect::<Result<Vec<_>, String>>()
                        .map_err(Error::QueryResponseFailure)?;

                    let columns = rows[0]
                        .columns()
                        .into_iter()
                        .map(|c: &DB::Column| c.try_into())
                        .collect::<Result<Vec<_>, String>>()
                        .map_err(Error::QueryResponseFailure)?;

                    let first_rows = rows
                        .iter()
                        .map(|r: &DB::Row| r.try_into())
                        .collect::<Result<Vec<_>, String>>()
                        .map_err(Error::QueryResponseFailure)?;

                    Ok(StreamDbResultSet::new(columns, first_rows, row_stream))
                }
                _ => Ok(StreamDbResultSet::new(vec![], vec![], row_stream)),
            }
        }
    }

    #[async_trait]
    impl<DB: Database> DbResultSet for StreamDbResultSet<'_, DB>
    where
        DB::Row: Row,
        DbRow: for<'a> TryFrom<&'a DB::Row, Error = String>,
    {
        async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
            info!("get_columns");
            Ok(self.columns.clone())
        }

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
            let mut rows = self.first_rows.lock().await;
            if rows.is_some() {
                info!("get_next - initial");
                let result = rows.clone();
                *rows = None;
                Ok(result)
            } else {
                info!("get_next");
                let mut stream = self.row_stream.lock().await;
                let next = stream.next().await;

                if let Some(rows) = next {
                    let mut values = Vec::with_capacity(rows.len());
                    for row in rows.into_iter() {
                        let row = row.map_err(|e| Error::QueryResponseFailure(e.to_string()))?;
                        let value = (&row).try_into().map_err(Error::QueryResponseFailure)?;
                        values.push(value);
                    }
                    Ok(Some(values))
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub(crate) trait QueryParamsBinder<'q, DB: Database> {
        fn bind_params(
            self,
            params: Vec<DbValue>,
        ) -> Result<sqlx::query::Query<'q, DB, <DB as HasArguments<'q>>::Arguments>, Error>;
    }
}

pub mod mysql {
    use crate::services::rdbms::sqlx_common::{
        PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
    };
    use crate::services::rdbms::types::{
        DbColumn, DbColumnType, DbColumnTypePrimitive, DbResultSet, DbRow, DbValue,
        DbValuePrimitive, Error,
    };
    use crate::services::rdbms::{RdbmsPoolConfig, RdbmsPoolKey};
    use async_trait::async_trait;
    use futures_util::stream::BoxStream;
    use golem_common::model::OwnedWorkerId;
    use sqlx::{Column, Pool, Row, TypeInfo};
    use std::sync::Arc;

    #[async_trait]
    pub trait Mysql {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), Error>;

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool;

        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<bool, Error>;

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, Error>;

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;
    }

    #[derive(Clone)]
    pub struct MysqlDefault {
        rdbms: Arc<SqlxRdbms<sqlx::MySql>>,
    }

    impl MysqlDefault {
        pub fn new(pool_config: RdbmsPoolConfig) -> Self {
            let rdbms = Arc::new(SqlxRdbms::new("mysql", pool_config));
            Self { rdbms }
        }
    }

    #[async_trait]
    impl Mysql for MysqlDefault {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), Error> {
            self.rdbms.create(worker_id, address).await
        }

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool {
            self.rdbms.exists(worker_id, address).await
        }

        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<bool, Error> {
            self.rdbms.remove(worker_id, address).await
        }

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, Error> {
            self.rdbms
                .execute(worker_id, address, statement, params)
                .await
        }

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
            self.rdbms
                .query(worker_id, address, statement, params)
                .await
        }
    }

    impl Default for MysqlDefault {
        fn default() -> Self {
            Self::new(RdbmsPoolConfig::default())
        }
    }

    #[async_trait]
    impl PoolCreator<sqlx::MySql> for RdbmsPoolKey {
        async fn create_pool(
            &self,
            config: &RdbmsPoolConfig,
        ) -> Result<Pool<sqlx::MySql>, sqlx::Error> {
            let address = self.address.clone();

            sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&address)
                .await
        }
    }

    #[async_trait]
    impl QueryExecutor for Pool<sqlx::MySql> {
        async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error> {
            let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
                sqlx::query(statement).bind_params(params)?;

            let result = query
                .execute(self)
                .await
                .map_err(|e| Error::QueryExecutionFailure(e.to_string()))?;
            Ok(result.rows_affected())
        }

        async fn query_stream(
            &self,
            statement: &str,
            params: Vec<DbValue>,
            batch: usize,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
            let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
                sqlx::query(statement.to_string().leak()).bind_params(params)?;

            let stream: BoxStream<Result<sqlx::mysql::MySqlRow, sqlx::Error>> = query.fetch(self);

            let response: StreamDbResultSet<sqlx::mysql::MySql> =
                StreamDbResultSet::create(stream, batch).await?;
            Ok(Arc::new(response))
        }
    }

    impl<'q> QueryParamsBinder<'q, sqlx::MySql>
        for sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>
    {
        fn bind_params(
            mut self,
            params: Vec<DbValue>,
        ) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, Error>
        {
            for param in params {
                self = bind_value(self, param)
                    .map_err(|e| Error::QueryParameterFailure(e.to_string()))?;
            }
            Ok(self)
        }
    }

    fn bind_value(
        query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>,
        value: DbValue,
    ) -> Result<sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>, String> {
        match value {
            DbValue::Primitive(v) => bind_value_primitive(query, v),
            DbValue::Array(_) => Err("Array param not supported".to_string()),
        }
    }

    fn bind_value_primitive(
        query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>,
        value: DbValuePrimitive,
    ) -> Result<sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>, String> {
        match value {
            DbValuePrimitive::Int8(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int16(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int32(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int64(v) => Ok(query.bind(v)),
            DbValuePrimitive::Decimal(v) => Ok(query.bind(v)),
            DbValuePrimitive::Float(v) => Ok(query.bind(v)),
            DbValuePrimitive::Boolean(v) => Ok(query.bind(v)),
            DbValuePrimitive::Text(v) => Ok(query.bind(v)),
            DbValuePrimitive::Blob(v) => Ok(query.bind(v)),
            DbValuePrimitive::Uuid(v) => Ok(query.bind(v)),
            DbValuePrimitive::Json(v) => Ok(query.bind(v)),
            // DbValuePrimitive::Xml(v) => Ok(query.bind(v)),
            DbValuePrimitive::Timestamp(v) => {
                Ok(query.bind(chrono::DateTime::from_timestamp_millis(v)))
            }
            // DbValuePrimitive::Interval(v) => Ok(query.bind(chrono::Duration::milliseconds(v))),
            DbValuePrimitive::DbNull => Ok(query.bind(None::<String>)),
            _ => Err(format!("Unsupported value: {:?}", value)),
        }
    }

    impl TryFrom<&sqlx::mysql::MySqlRow> for DbRow {
        type Error = String;

        fn try_from(value: &sqlx::mysql::MySqlRow) -> Result<Self, Self::Error> {
            let count = value.len();
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(get_db_value(index, value)?);
            }
            Ok(DbRow { values })
        }
    }

    fn get_db_value(index: usize, row: &sqlx::mysql::MySqlRow) -> Result<DbValue, String> {
        let column = &row.columns()[index];
        let type_name = column.type_info().name();
        let value = match type_name {
            mysql_type_name::BOOLEAN => {
                let v: Option<bool> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Boolean(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::TINYINT => {
                let v: Option<i8> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int8(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::SMALLINT => {
                let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int16(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::INT => {
                let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int32(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::BIGINT => {
                let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int64(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::FLOAT => {
                let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Float(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::DOUBLE => {
                let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Double(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::TEXT | mysql_type_name::VARCHAR | mysql_type_name::CHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Text(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::JSON => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Json(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            mysql_type_name::VARBINARY | mysql_type_name::BINARY | mysql_type_name::BLOB => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Blob(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            // mysql_type_name::UUID => {
            //     let v: Option<Uuid> = row.try_get(index).map_err(|e| e.to_string())?;
            //     match v {
            //         Some(v) => DbValue::Primitive(DbValuePrimitive::Uuid(v)),
            //         None => DbValue::Primitive(DbValuePrimitive::DbNull),
            //     }
            // }
            mysql_type_name::TIMESTAMP | mysql_type_name::DATETIME => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => {
                        DbValue::Primitive(DbValuePrimitive::Timestamp(v.timestamp_millis()))
                    }
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            _ => Err(format!("Unsupported type: {:?}", type_name))?,
        };
        Ok(value)
    }

    impl TryFrom<&sqlx::mysql::MySqlColumn> for DbColumn {
        type Error = String;

        fn try_from(value: &sqlx::mysql::MySqlColumn) -> Result<Self, Self::Error> {
            let ordinal = value.ordinal() as u64;
            let db_type: DbColumnType = value.type_info().try_into()?;
            let db_type_name = value.type_info().name().to_string();
            let name = value.name().to_string();
            Ok(DbColumn {
                ordinal,
                name,
                db_type,
                db_type_name,
            })
        }
    }

    impl TryFrom<&sqlx::mysql::MySqlTypeInfo> for DbColumnType {
        type Error = String;

        fn try_from(value: &sqlx::mysql::MySqlTypeInfo) -> Result<Self, Self::Error> {
            let type_name = value.name();

            match type_name {
                mysql_type_name::BOOLEAN => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Boolean))
                }
                mysql_type_name::TINYINT => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int8))
                }
                mysql_type_name::SMALLINT => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int16))
                }
                mysql_type_name::INT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int32)),
                mysql_type_name::BIGINT => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int64))
                }
                mysql_type_name::DECIMAL => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Decimal))
                }
                mysql_type_name::FLOAT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float)),
                mysql_type_name::DOUBLE => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Double))
                }
                // mysql_type_name::UUID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Uuid)),
                mysql_type_name::TEXT | mysql_type_name::VARCHAR | mysql_type_name::CHAR => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text))
                }
                mysql_type_name::JSON => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Json)),
                // mysql_type_name::XML => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Xml)),
                mysql_type_name::TIMESTAMP | mysql_type_name::DATETIME => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamp))
                }
                mysql_type_name::DATE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Date)),
                mysql_type_name::TIME => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Time)),
                mysql_type_name::VARBINARY | mysql_type_name::BINARY | mysql_type_name::BLOB => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Blob))
                }
                _ => Err(format!("Unsupported type: {:?}", value)),
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) mod mysql_type_name {
        pub(crate) const BOOLEAN: &str = "BOOLEAN";
        pub(crate) const TINYINT_UNSIGNED: &str = "TINYINT UNSIGNED";
        pub(crate) const SMALLINT_UNSIGNED: &str = "SMALLINT UNSIGNED";
        pub(crate) const INT_UNSIGNED: &str = "INT UNSIGNED";
        pub(crate) const MEDIUMINT_UNSIGNED: &str = "MEDIUMINT UNSIGNED";
        pub(crate) const BIGINT_UNSIGNED: &str = "BIGINT UNSIGNED";
        pub(crate) const TINYINT: &str = "TINYINT";
        pub(crate) const SMALLINT: &str = "SMALLINT";
        pub(crate) const INT: &str = "INT";
        pub(crate) const MEDIUMINT: &str = "MEDIUMINT";
        pub(crate) const BIGINT: &str = "BIGINT";
        pub(crate) const FLOAT: &str = "FLOAT";
        pub(crate) const DOUBLE: &str = "DOUBLE";
        pub(crate) const NULL: &str = "NULL";
        pub(crate) const TIMESTAMP: &str = "TIMESTAMP";
        pub(crate) const DATE: &str = "DATE";
        pub(crate) const TIME: &str = "TIME";
        pub(crate) const DATETIME: &str = "DATETIME";
        pub(crate) const YEAR: &str = "YEAR";
        pub(crate) const BIT: &str = "BIT";
        pub(crate) const ENUM: &str = "ENUM";
        pub(crate) const SET: &str = "SET";
        pub(crate) const DECIMAL: &str = "DECIMAL";
        pub(crate) const GEOMETRY: &str = "GEOMETRY";
        pub(crate) const JSON: &str = "JSON";

        pub(crate) const BINARY: &str = "BINARY";
        pub(crate) const VARBINARY: &str = "VARBINARY";

        pub(crate) const CHAR: &str = "CHAR";
        pub(crate) const VARCHAR: &str = "VARCHAR";

        pub(crate) const TINYBLOB: &str = "TINYBLOB";
        pub(crate) const TINYTEXT: &str = "TINYTEXT";

        pub(crate) const BLOB: &str = "BLOB";
        pub(crate) const TEXT: &str = "TEXT";

        pub(crate) const MEDIUMBLOB: &str = "MEDIUMBLOB";
        pub(crate) const MEDIUMTEXT: &str = "MEDIUMTEXT";

        pub(crate) const LONGBLOB: &str = "LONGBLOB";
        pub(crate) const LONGTEXT: &str = "LONGTEXT";
    }
}

pub mod postgres {
    use crate::services::rdbms::sqlx_common::{
        PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
    };
    use crate::services::rdbms::types::{
        DbColumn, DbColumnType, DbColumnTypePrimitive, DbResultSet, DbRow, DbValue,
        DbValuePrimitive, Error,
    };
    use crate::services::rdbms::{RdbmsPoolConfig, RdbmsPoolKey};
    use async_trait::async_trait;
    use futures_util::stream::BoxStream;
    use golem_common::model::OwnedWorkerId;
    use sqlx::{Column, Pool, Row, TypeInfo};
    use std::sync::Arc;
    use uuid::Uuid;

    #[async_trait]
    pub trait Postgres {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), Error>;

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool;

        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<bool, Error>;

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, Error>;

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;
    }

    #[derive(Clone)]
    pub struct PostgresDefault {
        rdbms: Arc<SqlxRdbms<sqlx::Postgres>>,
    }

    impl PostgresDefault {
        pub fn new(pool_config: RdbmsPoolConfig) -> Self {
            let rdbms = Arc::new(SqlxRdbms::new("postgres", pool_config));
            Self { rdbms }
        }
    }

    #[async_trait]
    impl Postgres for PostgresDefault {
        async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), Error> {
            self.rdbms.create(worker_id, address).await
        }

        async fn exists(&self, worker_id: &OwnedWorkerId, address: &str) -> bool {
            self.rdbms.exists(worker_id, address).await
        }

        async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<bool, Error> {
            self.rdbms.remove(worker_id, address).await
        }

        async fn execute(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<u64, Error> {
            self.rdbms
                .execute(worker_id, address, statement, params)
                .await
        }

        async fn query(
            &self,
            worker_id: &OwnedWorkerId,
            address: &str,
            statement: &str,
            params: Vec<DbValue>,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
            self.rdbms
                .query(worker_id, address, statement, params)
                .await
        }
    }

    impl Default for PostgresDefault {
        fn default() -> Self {
            Self::new(RdbmsPoolConfig::default())
        }
    }

    #[async_trait]
    impl PoolCreator<sqlx::Postgres> for RdbmsPoolKey {
        async fn create_pool(
            &self,
            config: &RdbmsPoolConfig,
        ) -> Result<Pool<sqlx::Postgres>, sqlx::Error> {
            let address = self.address.clone();

            sqlx::postgres::PgPoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&address)
                .await
        }
    }

    #[async_trait]
    impl QueryExecutor for sqlx::Pool<sqlx::Postgres> {
        async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error> {
            let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
                sqlx::query(statement).bind_params(params)?;

            let result = query
                .execute(self)
                .await
                .map_err(|e| Error::QueryExecutionFailure(e.to_string()))?;
            Ok(result.rows_affected())
        }

        async fn query_stream(
            &self,
            statement: &str,
            params: Vec<DbValue>,
            batch: usize,
        ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
            let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
                sqlx::query(statement.to_string().leak()).bind_params(params)?;

            let stream: BoxStream<Result<sqlx::postgres::PgRow, sqlx::Error>> = query.fetch(self);

            let response: StreamDbResultSet<sqlx::postgres::Postgres> =
                StreamDbResultSet::create(stream, batch).await?;
            Ok(Arc::new(response))
        }
    }

    impl<'q> QueryParamsBinder<'q, sqlx::Postgres>
        for sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>
    {
        fn bind_params(
            mut self,
            params: Vec<DbValue>,
        ) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, Error>
        {
            for param in params {
                self = bind_value(self, param)
                    .map_err(|e| Error::QueryParameterFailure(e.to_string()))?;
            }
            Ok(self)
        }
    }

    fn bind_value(
        query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        value: DbValue,
    ) -> Result<sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>, String> {
        match value {
            DbValue::Primitive(v) => bind_value_primitive(query, v),
            DbValue::Array(_) => Err("Array param not supported".to_string()),
        }
    }

    fn bind_value_primitive(
        query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        value: DbValuePrimitive,
    ) -> Result<sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>, String> {
        match value {
            DbValuePrimitive::Int8(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int16(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int32(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int64(v) => Ok(query.bind(v)),
            DbValuePrimitive::Decimal(v) => Ok(query.bind(v)),
            DbValuePrimitive::Float(v) => Ok(query.bind(v)),
            DbValuePrimitive::Boolean(v) => Ok(query.bind(v)),
            DbValuePrimitive::Text(v) => Ok(query.bind(v)),
            DbValuePrimitive::Blob(v) => Ok(query.bind(v)),
            DbValuePrimitive::Uuid(v) => Ok(query.bind(v)),
            DbValuePrimitive::Json(v) => Ok(query.bind(v)),
            DbValuePrimitive::Xml(v) => Ok(query.bind(v)),
            DbValuePrimitive::Timestamp(v) => {
                Ok(query.bind(chrono::DateTime::from_timestamp_millis(v)))
            }
            DbValuePrimitive::Interval(v) => Ok(query.bind(chrono::Duration::milliseconds(v))),
            DbValuePrimitive::DbNull => Ok(query.bind(None::<String>)),
            _ => Err(format!("Unsupported value: {:?}", value)),
        }
    }

    impl TryFrom<&sqlx::postgres::PgRow> for DbRow {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgRow) -> Result<Self, Self::Error> {
            let count = value.len();
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(get_db_value(index, value)?);
            }
            Ok(DbRow { values })
        }
    }

    fn get_db_value(index: usize, row: &sqlx::postgres::PgRow) -> Result<DbValue, String> {
        let column = &row.columns()[index];
        let type_name = column.type_info().name();
        let value = match type_name {
            pg_type_name::BOOL => {
                let v: Option<bool> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Boolean(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::INT2 => {
                let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int16(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::INT4 => {
                let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int32(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::INT8 => {
                let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int64(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::FLOAT4 => {
                let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Float(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::FLOAT8 => {
                let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Double(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::TEXT | pg_type_name::VARCHAR | pg_type_name::BPCHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Text(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::JSON => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Json(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::XML => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Json(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::BYTEA => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Blob(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::UUID => {
                let v: Option<Uuid> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Uuid(v)),
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            pg_type_name::TIMESTAMP | pg_type_name::TIMESTAMPTZ => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => {
                        DbValue::Primitive(DbValuePrimitive::Timestamp(v.timestamp_millis()))
                    }
                    None => DbValue::Primitive(DbValuePrimitive::DbNull),
                }
            }
            _ => Err(format!("Unsupported type: {:?}", type_name))?,
        };
        Ok(value)
    }

    impl TryFrom<&sqlx::postgres::PgColumn> for DbColumn {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgColumn) -> Result<Self, Self::Error> {
            let ordinal = value.ordinal() as u64;
            let db_type: DbColumnType = value.type_info().try_into()?;
            let db_type_name = value.type_info().name().to_string();
            let name = value.name().to_string();
            Ok(DbColumn {
                ordinal,
                name,
                db_type,
                db_type_name,
            })
        }
    }

    impl TryFrom<&sqlx::postgres::PgTypeInfo> for DbColumnType {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgTypeInfo) -> Result<Self, Self::Error> {
            let type_name = value.name();

            match type_name {
                pg_type_name::BOOL => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Boolean)),
                pg_type_name::INT2 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int16)),
                pg_type_name::INT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int32)),
                pg_type_name::INT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int64)),
                pg_type_name::NUMERIC => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Decimal))
                }
                pg_type_name::FLOAT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float)),
                pg_type_name::FLOAT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Double)),
                pg_type_name::UUID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Uuid)),
                pg_type_name::TEXT | pg_type_name::VARCHAR | pg_type_name::BPCHAR => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text))
                }
                pg_type_name::JSON => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Json)),
                pg_type_name::XML => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Xml)),
                pg_type_name::TIMESTAMP => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamp))
                }
                pg_type_name::DATE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Date)),
                pg_type_name::TIME => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Time)),
                pg_type_name::INTERVAL => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Interval))
                }
                pg_type_name::BYTEA => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Blob)),
                _ => Err(format!("Unsupported type: {:?}", value)),
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) mod pg_type_name {
        pub(crate) const BOOL: &str = "BOOL";
        pub(crate) const BYTEA: &str = "BYTEA";
        pub(crate) const CHAR: &str = "\"CHAR\"";
        pub(crate) const NAME: &str = "NAME";
        pub(crate) const INT8: &str = "INT8";
        pub(crate) const INT2: &str = "INT2";
        pub(crate) const INT4: &str = "INT4";
        pub(crate) const TEXT: &str = "TEXT";
        pub(crate) const OID: &str = "OID";
        pub(crate) const JSON: &str = "JSON";
        pub(crate) const JSON_ARRAY: &str = "JSON[]";
        pub(crate) const POINT: &str = "POINT";
        pub(crate) const LSEG: &str = "LSEG";
        pub(crate) const PATH: &str = "PATH";
        pub(crate) const BOX: &str = "BOX";
        pub(crate) const POLYGON: &str = "POLYGON";
        pub(crate) const LINE: &str = "LINE";
        pub(crate) const LINE_ARRAY: &str = "LINE[]";
        pub(crate) const CIDR: &str = "CIDR";
        pub(crate) const CIDR_ARRAY: &str = "CIDR[]";
        pub(crate) const FLOAT4: &str = "FLOAT4";
        pub(crate) const FLOAT8: &str = "FLOAT8";
        pub(crate) const UNKNOWN: &str = "UNKNOWN";
        pub(crate) const CIRCLE: &str = "CIRCLE";
        pub(crate) const CIRCLE_ARRAY: &str = "CIRCLE[]";
        pub(crate) const MACADDR8: &str = "MACADDR8";
        pub(crate) const MACADDR8_ARRAY: &str = "MACADDR8[]";
        pub(crate) const MACADDR: &str = "MACADDR";
        pub(crate) const INET: &str = "INET";
        pub(crate) const BOOL_ARRAY: &str = "BOOL[]";
        pub(crate) const BYTEA_ARRAY: &str = "BYTEA[]";
        pub(crate) const CHAR_ARRAY: &str = "\"CHAR\"[]";
        pub(crate) const NAME_ARRAY: &str = "NAME[]";
        pub(crate) const INT2_ARRAY: &str = "INT2[]";
        pub(crate) const INT4_ARRAY: &str = "INT4[]";
        pub(crate) const TEXT_ARRAY: &str = "TEXT[]";
        pub(crate) const BPCHAR_ARRAY: &str = "CHAR[]";
        pub(crate) const VARCHAR_ARRAY: &str = "VARCHAR[]";
        pub(crate) const INT8_ARRAY: &str = "INT8[]";
        pub(crate) const POINT_ARRAY: &str = "POINT[]";
        pub(crate) const LSEG_ARRAY: &str = "LSEG[]";
        pub(crate) const PATH_ARRAY: &str = "PATH[]";
        pub(crate) const BOX_ARRAY: &str = "BOX[]";
        pub(crate) const FLOAT4_ARRAY: &str = "FLOAT4[]";
        pub(crate) const FLOAT8_ARRAY: &str = "FLOAT8[]";
        pub(crate) const POLYGON_ARRAY: &str = "POLYGON[]";
        pub(crate) const OID_ARRAY: &str = "OID[]";
        pub(crate) const MACADDR_ARRAY: &str = "MACADDR[]";
        pub(crate) const INET_ARRAY: &str = "INET[]";
        pub(crate) const BPCHAR: &str = "CHAR";
        pub(crate) const VARCHAR: &str = "VARCHAR";
        pub(crate) const DATE: &str = "DATE";
        pub(crate) const TIME: &str = "TIME";
        pub(crate) const TIMESTAMP: &str = "TIMESTAMP";
        pub(crate) const TIMESTAMP_ARRAY: &str = "TIMESTAMP[]";
        pub(crate) const DATE_ARRAY: &str = "DATE[]";
        pub(crate) const TIME_ARRAY: &str = "TIME[]";
        pub(crate) const TIMESTAMPTZ: &str = "TIMESTAMPTZ";
        pub(crate) const TIMESTAMPTZ_ARRAY: &str = "TIMESTAMPTZ[]";
        pub(crate) const INTERVAL: &str = "INTERVAL";
        pub(crate) const INTERVAL_ARRAY: &str = "INTERVAL[]";
        pub(crate) const NUMERIC_ARRAY: &str = "NUMERIC[]";
        pub(crate) const TIMETZ: &str = "TIMETZ";
        pub(crate) const TIMETZ_ARRAY: &str = "TIMETZ[]";
        pub(crate) const BIT: &str = "BIT";
        pub(crate) const BIT_ARRAY: &str = "BIT[]";
        pub(crate) const VARBIT: &str = "VARBIT";
        pub(crate) const VARBIT_ARRAY: &str = "VARBIT[]";
        pub(crate) const NUMERIC: &str = "NUMERIC";
        pub(crate) const RECORD: &str = "RECORD";
        pub(crate) const RECORD_ARRAY: &str = "RECORD[]";
        pub(crate) const UUID: &str = "UUID";
        pub(crate) const UUID_ARRAY: &str = "UUID[]";
        pub(crate) const JSONB: &str = "JSONB";
        pub(crate) const JSONB_ARRAY: &str = "JSONB[]";
        pub(crate) const INT4RANGE: &str = "INT4RANGE";
        pub(crate) const INT4RANGE_ARRAY: &str = "INT4RANGE[]";
        pub(crate) const NUMRANGE: &str = "NUMRANGE";
        pub(crate) const NUMRANGE_ARRAY: &str = "NUMRANGE[]";
        pub(crate) const TSRANGE: &str = "TSRANGE";
        pub(crate) const TSRANGE_ARRAY: &str = "TSRANGE[]";
        pub(crate) const TSTZRANGE: &str = "TSTZRANGE";
        pub(crate) const TSTZRANGE_ARRAY: &str = "TSTZRANGE[]";
        pub(crate) const DATERANGE: &str = "DATERANGE";
        pub(crate) const DATERANGE_ARRAY: &str = "DATERANGE[]";
        pub(crate) const INT8RANGE: &str = "INT8RANGE";
        pub(crate) const INT8RANGE_ARRAY: &str = "INT8RANGE[]";
        pub(crate) const JSONPATH: &str = "JSONPATH";
        pub(crate) const JSONPATH_ARRAY: &str = "JSONPATH[]";
        pub(crate) const MONEY: &str = "MONEY";
        pub(crate) const MONEY_ARRAY: &str = "MONEY[]";
        pub(crate) const VOID: &str = "VOID";
        pub(crate) const XML: &str = "XML";
    }
}

pub mod types {
    use async_trait::async_trait;
    use bigdecimal::BigDecimal;
    use std::fmt::Display;
    use std::sync::{Arc, Mutex};
    use tracing::info;
    use uuid::Uuid;

    #[async_trait]
    pub trait DbResultSet {
        async fn get_columns(&self) -> Result<Vec<DbColumn>, Error>;

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error>;
    }

    #[derive(Clone, Debug)]
    pub struct SimpleDbResultSet {
        columns: Vec<DbColumn>,
        rows: Arc<Mutex<Option<Vec<DbRow>>>>,
    }

    impl SimpleDbResultSet {
        pub fn new(columns: Vec<DbColumn>, rows: Option<Vec<DbRow>>) -> Self {
            Self {
                columns,
                rows: Arc::new(Mutex::new(rows)),
            }
        }
    }

    #[async_trait]
    impl DbResultSet for SimpleDbResultSet {
        async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
            info!("get_columns");
            Ok(self.columns.clone())
        }

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
            let rows = self.rows.lock().unwrap().clone();
            info!("get_next {}", rows.is_some());
            if rows.is_some() {
                *self.rows.lock().unwrap() = None;
            }
            Ok(rows)
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct EmptyDbResultSet {}

    #[async_trait]
    impl DbResultSet for EmptyDbResultSet {
        async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
            info!("get_columns");
            Ok(vec![])
        }

        async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
            info!("get_next");
            Ok(None)
        }
    }

    #[derive(Clone, Debug)]
    pub enum DbColumnTypePrimitive {
        Int8,
        Int16,
        Int32,
        Int64,
        Float,
        Double,
        Decimal,
        Boolean,
        Timestamp,
        Date,
        Time,
        Interval,
        Text,
        Blob,
        Json,
        Xml,
        Uuid,
    }

    #[derive(Clone, Debug)]
    pub enum DbColumnType {
        Primitive(DbColumnTypePrimitive),
        Array(DbColumnTypePrimitive),
    }

    #[derive(Clone, Debug)]
    pub enum DbValuePrimitive {
        Int8(i8),
        Int16(i16),
        Int32(i32),
        Int64(i64),
        Float(f32),
        Double(f64),
        Decimal(BigDecimal),
        Boolean(bool),
        Timestamp(i64),
        Date(i64),
        Time(i64),
        Interval(i64),
        Text(String),
        Blob(Vec<u8>),
        Json(String),
        Xml(String),
        Uuid(Uuid),
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
    pub struct DbColumn {
        pub ordinal: u64,
        pub name: String,
        pub db_type: DbColumnType,
        pub db_type_name: String,
    }

    // #[derive(Clone, Debug)]
    // pub struct DbColumnTypeMeta {
    //     pub name: String,
    //     pub db_type: DbColumnType,
    //     pub db_type_flags: HashSet<DbColumnTypeFlag>,
    //     pub foreign_key: Option<String>,
    // }
    //
    // #[derive(Clone, Debug)]
    // pub enum DbColumnTypeFlag {
    //     PrimaryKey,
    //     ForeignKey,
    //     Unique,
    //     Nullable,
    //     Generated,
    //     AutoIncrement,
    //     DefaultValue,
    //     Indexed,
    // }

    #[derive(Clone, Debug)]
    pub enum Error {
        ConnectionFailure(String),
        QueryParameterFailure(String),
        QueryExecutionFailure(String),
        QueryResponseFailure(String),
        Other(String),
    }

    impl Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Error::ConnectionFailure(msg) => write!(f, "ConnectionFailure: {}", msg),
                Error::QueryParameterFailure(msg) => write!(f, "QueryParameterFailure: {}", msg),
                Error::QueryExecutionFailure(msg) => write!(f, "QueryExecutionFailure: {}", msg),
                Error::QueryResponseFailure(msg) => write!(f, "QueryResponseFailure: {}", msg),
                Error::Other(msg) => write!(f, "Other: {}", msg),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::services::rdbms::types::{DbRow, DbValue, DbValuePrimitive};
    use crate::services::rdbms::RdbmsService;
    use crate::services::rdbms::RdbmsServiceDefault;
    use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, WorkerId};
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

        let columns = result.unwrap().get_columns().await.unwrap();
        // println!("columns: {columns:?}");
        assert!(columns.len() > 0);

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

        for _ in 0..100 {
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
        }

        let result = rdbms_service
            .postgres()
            .query(&worker_id, &address, "SELECT * from components", vec![])
            .await;

        assert!(result.is_ok());

        let result = result.unwrap();

        let columns = result.get_columns().await.unwrap();
        // println!("columns: {columns:?}");
        assert!(columns.len() > 0);

        let mut rows: Vec<DbRow> = vec![];

        loop {
            match result.get_next().await.unwrap() {
                Some(vs) => rows.extend(vs),
                None => break,
            }
        }
        // println!("rows: {rows:?}");
        assert!(rows.len() >= 100);
        println!("rows: {}", rows.len());

        let result = rdbms_service.postgres().remove(&worker_id, &address).await;

        assert!(result.is_ok_and(|v| v));

        let exists = rdbms_service.postgres().exists(&worker_id, &address).await;

        assert!(!exists);
    }
}

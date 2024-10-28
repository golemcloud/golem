use async_trait::async_trait;
use golem_common::model::OwnedWorkerId;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

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
        Self::new(Arc::new(PostgresDefault {}))
    }
}

impl RdbmsService for RdbmsServiceDefault {
    fn postgres(&self) -> Arc<dyn Postgres + Send + Sync> {
        self.postgres.clone()
    }
}

#[async_trait]
pub trait Postgres {
    async fn create(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String>;
    async fn remove(&self, worker_id: &OwnedWorkerId, address: &str) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct PostgresDefault {}

#[async_trait]
impl Postgres for PostgresDefault {
    async fn create(&self, _worker_id: &OwnedWorkerId, _address: &str) -> Result<(), String> {
        Ok(())
    }

    async fn remove(&self, _worker_id: &OwnedWorkerId, _address: &str) -> Result<(), String> {
        Ok(())
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

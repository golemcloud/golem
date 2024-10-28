use async_trait::async_trait;
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
    async fn create(&self, address: &str) -> Result<(), String>;
    async fn remove(&self, address: &str) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct PostgresDefault {}

#[async_trait]
impl Postgres for PostgresDefault {
    async fn create(&self, _address: &str) -> Result<(), String> {
        Ok(())
    }

    async fn remove(&self, _address: &str) -> Result<(), String> {
        Ok(())
    }
}

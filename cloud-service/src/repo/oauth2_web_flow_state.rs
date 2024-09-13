use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use sqlx::{Database, Pool};
use uuid::Uuid;

use super::{token::TokenRecord, RepoError};

#[derive(Debug, Clone)]
pub enum LinkedTokenState {
    Linked(LinkedToken),
    /// Token has not been linked yet
    Pending,
    /// Token does not exist
    NotFound,
}

#[derive(Debug, Clone)]
pub struct LinkedToken {
    pub token: TokenRecord,
    pub metadata: Vec<u8>,
}

impl From<MaybeToken> for Option<LinkedToken> {
    fn from(value: MaybeToken) -> Self {
        match value {
            MaybeToken {
                id: Some(id),
                account_id: Some(account_id),
                secret: Some(secret),
                created_at: Some(created_at),
                expires_at: Some(expires_at),
                metadata: Some(metadata),
            } => Some(LinkedToken {
                token: TokenRecord {
                    id,
                    account_id,
                    secret,
                    created_at,
                    expires_at,
                },
                metadata,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct MaybeToken {
    id: Option<Uuid>,
    account_id: Option<String>,
    secret: Option<Uuid>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    metadata: Option<Vec<u8>>,
}

impl From<Option<MaybeToken>> for LinkedTokenState {
    fn from(value: Option<MaybeToken>) -> Self {
        match value {
            Some(maybe_token) => {
                if maybe_token.id.is_none() && maybe_token.metadata.is_some() {
                    LinkedTokenState::Pending
                } else {
                    let token: Option<LinkedToken> = maybe_token.into();
                    match token {
                        Some(token) => LinkedTokenState::Linked(token),
                        None => LinkedTokenState::NotFound,
                    }
                }
            }
            None => LinkedTokenState::NotFound,
        }
    }
}

#[async_trait]
pub trait OAuth2WebFlowStateRepo {
    async fn generate_temp_token_state(&self, metadata: &[u8]) -> Result<String, RepoError>;
    async fn valid_temp_token(&self, state: &str) -> Result<bool, RepoError>;
    async fn link_temp_token(
        &self,
        token_id: &Uuid,
        state: &str,
    ) -> Result<Option<LinkedToken>, RepoError>;
    async fn get_temp_token(&self, state: &str) -> Result<LinkedTokenState, RepoError>;
    async fn delete_expired_states(&self) -> Result<(), RepoError>;
}

static EXPIRATION_TIME: std::time::Duration = std::time::Duration::from_secs(60 * 5);

pub struct DbOAuth2FlowState<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbOAuth2FlowState<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl OAuth2WebFlowStateRepo for DbOAuth2FlowState<sqlx::Postgres> {
    async fn generate_temp_token_state(&self, metadata: &[u8]) -> Result<String, RepoError> {
        let state = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
              INSERT INTO oauth2_web_flow_state
                (oauth2_state, metadata)
              VALUES
                ($1, $2)
            "#,
        )
        .bind(&state)
        .bind(metadata)
        .execute(self.db_pool.deref())
        .await?;

        Ok(state)
    }

    async fn valid_temp_token(&self, state: &str) -> Result<bool, RepoError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM oauth2_web_flow_state WHERE oauth2_state = $1")
                .bind(state)
                .fetch_one(self.db_pool.deref())
                .await?;

        Ok(count > 0)
    }

    async fn link_temp_token(
        &self,
        token_id: &Uuid,
        state: &str,
    ) -> Result<Option<LinkedToken>, RepoError> {
        let result = sqlx::query_as::<_, MaybeToken>(
            r#"
                WITH updated AS (
                    UPDATE oauth2_web_flow_state
                    SET token_id = $1
                    WHERE oauth2_state = $2
                    RETURNING token_id, metadata
                )
                SELECT t.id,
                       t.account_id,
                       t.secret,
                       t.created_at::timestamptz,
                       t.expires_at::timestamptz,
                       updated.metadata
                FROM updated
                JOIN tokens t ON t.id = updated.token_id
                "#,
        )
        .bind(token_id)
        .bind(state)
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.and_then(|token| token.into()))
    }

    async fn get_temp_token(&self, state: &str) -> Result<LinkedTokenState, RepoError> {
        let result: Option<MaybeToken> = sqlx::query_as(
            r#"
            SELECT t.id,
                   t.account_id,
                   t.secret,
                   t.created_at::timestamptz,
                   t.expires_at::timestamptz,
                   flow_state.metadata
            FROM oauth2_web_flow_state flow_state
            LEFT JOIN tokens t ON t.id = flow_state.token_id
            WHERE flow_state.oauth2_state = $1
            "#,
        )
        .bind(state)
        .fetch_optional(self.db_pool.deref())
        .await?;

        let token_state: LinkedTokenState = result.into();

        Ok(token_state)
    }

    async fn delete_expired_states(&self) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM oauth2_web_flow_state WHERE created_at < $1")
            .bind(chrono::Utc::now() - EXPIRATION_TIME)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }
}

#[async_trait]
impl OAuth2WebFlowStateRepo for DbOAuth2FlowState<sqlx::Sqlite> {
    async fn generate_temp_token_state(&self, metadata: &[u8]) -> Result<String, RepoError> {
        let state = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO oauth2_web_flow_state (oauth2_state, metadata) VALUES ($1, $2)")
            .bind(&state)
            .bind(metadata)
            .execute(self.db_pool.deref())
            .await?;

        Ok(state)
    }

    async fn valid_temp_token(&self, state: &str) -> Result<bool, RepoError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM oauth2_web_flow_state WHERE oauth2_state = $1")
                .bind(state)
                .fetch_one(self.db_pool.deref())
                .await?;

        Ok(count > 0)
    }

    async fn link_temp_token(
        &self,
        token_id: &Uuid,
        state: &str,
    ) -> Result<Option<LinkedToken>, RepoError> {
        let result = sqlx::query_as::<_, MaybeToken>(
            r#"
            WITH updated AS (
                UPDATE oauth2_web_flow_state
                SET token_id = $1
                WHERE oauth2_state = $2
                RETURNING token_id, metadata
            )
            SELECT t.id, t.account_id, t.secret, t.created_at, t.expires_at, updated.metadata
            FROM updated
            JOIN tokens t ON t.id = updated.token_id
            "#,
        )
        .bind(token_id)
        .bind(state)
        .fetch_optional(self.db_pool.deref())
        .await?;

        Ok(result.and_then(|token| token.into()))
    }

    async fn get_temp_token(&self, state: &str) -> Result<LinkedTokenState, RepoError> {
        let result: Option<MaybeToken> = sqlx::query_as(
            r#"
            SELECT t.id, t.account_id, t.secret, t.created_at, t.expires_at, flow_state.metadata
            FROM oauth2_web_flow_state flow_state
            LEFT JOIN tokens t ON t.id = flow_state.token_id
            WHERE flow_state.oauth2_state = $1
            "#,
        )
        .bind(state)
        .fetch_optional(self.db_pool.deref())
        .await?;

        let token_state: LinkedTokenState = result.into();

        Ok(token_state)
    }

    async fn delete_expired_states(&self) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM oauth2_web_flow_state WHERE created_at < $1")
            .bind(chrono::Utc::now() - EXPIRATION_TIME)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }
}

// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::{SafeDisplay, error_forwarders};
use sqlx::error::ErrorKind;

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("Unique violation repository error: {0}")]
    UniqueViolation(String),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl RepoError {
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, RepoError::UniqueViolation(_))
    }
}

error_forwarders!(RepoError);

impl From<sqlx::Error> for RepoError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db_error) = error.as_database_error()
            && db_error.kind() == ErrorKind::UniqueViolation
        {
            RepoError::UniqueViolation(db_error.to_string())
        } else {
            RepoError::InternalError(error.into())
        }
    }
}

impl SafeDisplay for RepoError {
    fn to_safe_string(&self) -> String {
        match self {
            RepoError::InternalError(_) => "Internal repository error".to_string(),
            RepoError::UniqueViolation(_) => {
                "Internal repository error (unique key violation)".to_string()
            }
        }
    }
}

pub type RepoResult<T> = Result<T, RepoError>;

pub type BusinessResult<T, E> = Result<Result<T, E>, RepoError>;

pub trait ResultExt<T> {
    fn none_on_unique_violation(self) -> RepoResult<Option<T>>;

    fn false_on_unique_violation(self) -> RepoResult<bool>;

    fn to_business_result_on_unique_violation<F, E>(
        self,
        to_business_error: F,
    ) -> BusinessResult<T, E>
    where
        F: FnOnce() -> E;

    fn to_custom_result_on_unique_violation<E: From<RepoError>>(
        self,
        business_error: E,
    ) -> Result<T, E>;
}

impl<T> ResultExt<T> for RepoResult<T> {
    fn none_on_unique_violation(self) -> RepoResult<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn false_on_unique_violation(self) -> RepoResult<bool> {
        match self {
            Ok(_) => Ok(true),
            Err(err) if err.is_unique_violation() => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn to_business_result_on_unique_violation<F, E>(
        self,
        to_business_error: F,
    ) -> BusinessResult<T, E>
    where
        F: FnOnce() -> E,
    {
        match self {
            Ok(value) => Ok(Ok(value)),
            Err(err) if err.is_unique_violation() => Ok(Err(to_business_error())),
            Err(err) => Err(err),
        }
    }

    fn to_custom_result_on_unique_violation<E: From<RepoError>>(
        self,
        business_error: E,
    ) -> Result<T, E> {
        match self {
            Ok(value) => Ok(value),
            Err(err) if err.is_unique_violation() => Err(business_error),
            Err(err) => Err(err.into()),
        }
    }
}

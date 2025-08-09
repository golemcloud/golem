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

use crate::db::TxError;
use golem_common::SafeDisplay;
use sqlx::error::ErrorKind;

pub mod plugin_installation;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum RepoError {
    #[error("Internal repository error: {0}")]
    Internal(String),
    #[error("Unique violation repository error: {0}")]
    UniqueViolation(String),
}

impl RepoError {
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, RepoError::UniqueViolation(_))
    }
}

impl From<sqlx::Error> for RepoError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db_error) = error.as_database_error() {
            if db_error.kind() == ErrorKind::UniqueViolation {
                RepoError::UniqueViolation(db_error.to_string())
            } else {
                RepoError::Internal(db_error.to_string())
            }
        } else {
            RepoError::Internal(error.to_string())
        }
    }
}

impl SafeDisplay for RepoError {
    fn to_safe_string(&self) -> String {
        match self {
            RepoError::Internal(_) => "Internal repository error".to_string(),
            RepoError::UniqueViolation(_) => {
                "Internal repository error (unique key violation)".to_string()
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, RepoError>;

pub type BusinessResult<T, E> = std::result::Result<std::result::Result<T, E>, RepoError>;

pub trait ResultExt<T> {
    fn none_on_unique_violation(self) -> Result<Option<T>>;

    fn false_on_unique_violation(self) -> Result<bool>;

    fn to_business_result_on_unique_violation<F, E>(
        self,
        to_business_error: F,
    ) -> BusinessResult<T, E>
    where
        F: FnOnce() -> E;
}

impl<T> ResultExt<T> for Result<T> {
    fn none_on_unique_violation(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn false_on_unique_violation(self) -> Result<bool> {
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
}

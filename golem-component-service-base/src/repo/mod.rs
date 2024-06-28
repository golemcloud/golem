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

pub mod component;

use std::fmt::Display;

#[derive(Debug)]
pub enum RepoError {
    Internal(String),
}

impl From<sqlx::Error> for RepoError {
    fn from(error: sqlx::Error) -> Self {
        RepoError::Internal(error.to_string())
    }
}

impl Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::Internal(error) => write!(f, "{}", error),
        }
    }
}

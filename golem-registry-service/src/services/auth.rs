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

use golem_common::{into_internal_error, SafeDisplay};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error)
}

impl SafeDisplay for AuthError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

into_internal_error!(AuthError);

pub struct AuthService {

}

impl AuthService {
    pub fn new() -> Self {
        Self { }
    }
}

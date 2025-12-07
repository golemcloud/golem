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

use crate::debug_session::DebugSessions;
use crate::services::auth::AuthService;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdditionalDeps {
    auth_service: Arc<dyn AuthService>,
    debug_session: Arc<dyn DebugSessions>,
}

impl AdditionalDeps {
    pub fn new(auth_service: Arc<dyn AuthService>, debug_session: Arc<dyn DebugSessions>) -> Self {
        Self {
            auth_service,
            debug_session,
        }
    }

    pub fn auth_service(&self) -> Arc<dyn AuthService + Sync + Send> {
        self.auth_service.clone()
    }

    pub fn debug_session(&self) -> Arc<dyn DebugSessions + Sync + Send> {
        self.debug_session.clone()
    }
}

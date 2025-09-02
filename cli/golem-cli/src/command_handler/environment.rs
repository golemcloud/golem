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

use crate::command::environment::EnvironmentSubcommand;
use crate::context::Context;
use crate::model::environment::{EnvironmentReference, ResolvedEnvironmentIdentity};
use std::sync::Arc;

pub struct EnvironmentCommandHandler {
    // TODO: atomic
    #[allow(unused)]
    ctx: Arc<Context>,
}

impl EnvironmentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, _subcommand: EnvironmentSubcommand) -> anyhow::Result<()> {
        // TODO: atomic
        todo!()
    }

    pub async fn select_environment(
        &self,
        _environment: Option<&EnvironmentReference>,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        // TODO: atomic
        todo!()
    }
}

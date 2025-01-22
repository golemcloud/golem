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

use crate::model::{GolemError, GolemResult};
use clap::Args;
use golem_ui_service::UiService;
// use std::net::SocketAddr;

#[derive(Args, Debug)]
pub struct UiCommand {
    /// Server port
    #[arg(long, default_value = "3000")]
    port: u16,

    /// Server host
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// API base URL
    #[arg(long, default_value = "http://localhost:9881")]
    api_url: String,

    /// Development mode
    #[arg(long)]
    dev: bool,
}

impl UiCommand {
    pub async fn handle(self) -> Result<GolemResult, GolemError> {
        // let addr = SocketAddr::new(
        //     self.host.parse().map_err(|e| GolemError(format!("Invalid host address: {}", e)))?,
        //     self.port,
        // );

        let service = UiService::new(golem_ui_service::CliArgs {
            port: self.port,
            host: self.host,
            api_url: self.api_url,
            dev: self.dev,
        });

        service.run().await.map_err(|e| GolemError(e.to_string()))?;

        Ok(GolemResult::Str("UI server stopped".to_string()))
    }
}

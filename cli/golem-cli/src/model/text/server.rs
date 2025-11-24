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

use crate::config::{NamedProfile, BUILTIN_LOCAL_URL, CLOUD_URL};
use crate::model::app_raw::{BuiltinServer, Environment, Server};
use colored::Colorize;

pub trait ToFormattedServerContext {
    fn to_formatted_server_context(&self) -> String;
}

impl ToFormattedServerContext for NamedProfile {
    fn to_formatted_server_context(&self) -> String {
        if self.name.is_builtin_local() {
            local_builtin()
        } else if self.name.is_builtin_cloud() {
            cloud_builtin()
        } else {
            match &self.profile.custom_url {
                Some(custom_url) => custom_url.as_str().underline().to_string(),
                None => CLOUD_URL.underline().to_string(),
            }
        }
    }
}

impl ToFormattedServerContext for Environment {
    fn to_formatted_server_context(&self) -> String {
        match &self.server {
            Some(Server::Builtin(BuiltinServer::Local)) | None => local_builtin(),
            Some(Server::Builtin(BuiltinServer::Cloud)) => cloud_builtin(),
            Some(Server::Custom(custom_server)) => {
                custom_server.url.as_str().underline().to_string()
            }
        }
    }
}

fn local_builtin() -> String {
    format!("local - builtin ({})", BUILTIN_LOCAL_URL.underline())
}

fn cloud_builtin() -> String {
    format!("cloud - builtin ({})", CLOUD_URL.underline())
}

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

mod deployment_context;
mod http_parameter_conversion;
mod read;
mod route_compilation;
mod routes;
mod write;

pub use self::read::{DeploymentError, DeploymentService};
pub use self::routes::{DeployedRoutesError, DeployedRoutesService};
pub use self::write::{DeploymentWriteError, DeploymentWriteService};

macro_rules! ok_or_continue {
    ($expr:expr, $errors:ident) => {{
        match ($expr) {
            Ok(v) => v,
            Err(e) => {
                $errors.push(e);
                continue;
            }
        }
    }};
}
use ok_or_continue;

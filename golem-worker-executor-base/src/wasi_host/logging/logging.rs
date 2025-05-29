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

use tracing::debug;
use wasmtime_wasi::{WasiImpl, WasiView};

use crate::preview2::wasi::logging::logging::{Host, Level};

impl<T: WasiView> Host for WasiImpl<T> {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        debug!(
            "logging::logging::log called: {:?} [{}] {}",
            level, context, message
        );
        Ok(())
    }
}

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

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use test_r::{tag_suite, test_dep};

test_r::enable!();

mod app;
mod mcp;
mod stubgen;

tag_suite!(stubgen, group1);
tag_suite!(mcp, group1);

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::test(
            "golem-cli-integration-tests",
        ));
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

static WORKSPACE_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn workspace_path() -> PathBuf {
    WORKSPACE_PATH
        .get_or_init(|| {
            crate_path()
                .join("../..")
                .canonicalize()
                .expect("Failed to canonicalize workspace path")
        })
        .clone()
}

static CRATE_PATH: &str = env!("CARGO_MANIFEST_DIR");

pub fn crate_path() -> &'static Path {
    Path::new(CRATE_PATH)
}

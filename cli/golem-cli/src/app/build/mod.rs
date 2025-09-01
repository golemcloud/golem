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

use crate::app::build::add_metadata::add_metadata_to_selected_components;
use crate::app::build::componentize::componentize;
use crate::app::build::gen_rpc::gen_rpc;
use crate::app::build::link::link;
use crate::app::context::ApplicationContext;
use crate::fs;
use crate::log::{log_warn_action, LogColorize};
use crate::model::app::AppBuildStep;
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::debug;
use walkdir::WalkDir;

pub mod add_metadata;
pub mod clean;
pub mod command;
pub mod componentize;
pub mod gen_rpc;
pub mod link;
pub mod task_result_marker;

pub async fn build_app(ctx: &mut ApplicationContext) -> anyhow::Result<()> {
    if ctx.config.should_run_step(AppBuildStep::GenRpc) {
        gen_rpc(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::Componentize) {
        componentize(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::Link) {
        link(ctx).await?;
    }
    if ctx.config.should_run_step(AppBuildStep::AddMetadata) {
        add_metadata_to_selected_components(ctx).await?;
    }

    Ok(())
}

fn env_var_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|flag| {
            let flag = flag.to_lowercase();
            flag.starts_with("t") || flag == "1"
        })
        .unwrap_or_default()
}

/// Similar to std::env::vars() but silently drops invalid env vars instead of panicing.
/// Additionally, will ignore all env vars containing data incompatible with envsubst.
fn valid_env_vars() -> HashMap<String, String> {
    let mut result = HashMap::new();

    fn validate(val: OsString) -> Option<String> {
        let forbidden = &["$", "{", "}"];

        let str = val.into_string().ok()?;
        for c in forbidden {
            if str.contains(c) {
                return None;
            }
        }
        Some(str)
    }

    for (k, v) in std::env::vars_os() {
        if let (Some(k), Some(v)) = (validate(k.clone()), validate(v)) {
            result.insert(k, v);
        } else {
            debug!(
                "Env var `{}` contains invalid data and will be ignored",
                k.to_string_lossy()
            )
        }
    }
    result
}

fn delete_path_logged(context: &str, path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        log_warn_action(
            "Deleting",
            format!("{} {}", context, path.log_color_highlight()),
        );
        fs::remove(path).with_context(|| {
            anyhow!(
                "Failed to delete {}, path: {}",
                context.log_color_highlight(),
                path.log_color_highlight()
            )
        })?;
    }
    Ok(())
}

fn is_up_to_date<S, T, FS, FT>(skip_check: bool, sources: FS, targets: FT) -> bool
where
    S: Debug + IntoIterator<Item = PathBuf>,
    T: Debug + IntoIterator<Item = PathBuf>,
    FS: FnOnce() -> S,
    FT: FnOnce() -> T,
{
    if skip_check {
        debug!("skipping up-to-date check");
        return false;
    }

    fn max_modified(path: &Path) -> Option<SystemTime> {
        let mut max_modified: Option<SystemTime> = None;
        let mut update_max_modified = |modified: SystemTime| {
            if max_modified.is_none_or(|max_mod| max_mod.cmp(&modified) == Ordering::Less) {
                max_modified = Some(modified)
            }
        };

        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_dir() {
                WalkDir::new(path)
                    .into_iter()
                    .filter_map(|entry| entry.ok().and_then(|entry| entry.metadata().ok()))
                    .filter(|metadata| !metadata.is_dir())
                    .filter_map(|metadata| metadata.modified().ok())
                    .for_each(update_max_modified)
            } else if let Ok(modified) = metadata.modified() {
                update_max_modified(modified)
            }
        }

        debug!(
            path = %path.display(),
            max_modified = max_modified.map(|d| DateTime::<Utc>::from(d).to_string()),
            "max modified"
        );

        max_modified
    }

    fn max_modified_short_circuit_on_missing<I: IntoIterator<Item = PathBuf>>(
        paths: I,
    ) -> Option<SystemTime> {
        // Using Result and collect for short-circuit on any missing mod time
        paths
            .into_iter()
            .map(|path| max_modified(path.as_path()).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|mod_times| mod_times.into_iter().max().ok_or(()))
            .ok()
    }

    let targets = targets();
    debug!(targets=?targets, "collected targets");

    let max_target_modified = max_modified_short_circuit_on_missing(targets);

    let max_target_modified = match max_target_modified {
        Some(modified) => modified,
        None => {
            debug!("missing targets, not up-to-date");
            return false;
        }
    };

    let sources = sources();
    debug!(source=?sources, "collected sources");

    let max_source_modified = max_modified_short_circuit_on_missing(sources);

    match max_source_modified {
        Some(max_source_modified) => {
            let up_to_date = max_source_modified.cmp(&max_target_modified) == Ordering::Less;
            debug!(up_to_date, "up to date result based on timestamps");
            up_to_date
        }
        None => {
            debug!("missing sources, not up-to-date");
            false
        }
    }
}

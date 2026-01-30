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

use crate::app::build::task_result_marker::{TaskResultMarker, TaskResultMarkerHashSource};
use crate::app::context::BuildContext;
use crate::fs;
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::debug;
use walkdir::WalkDir;

pub fn is_up_to_date<S, T, SP, TP, FS, FT>(skip_check: bool, sources: FS, targets: FT) -> bool
where
    S: Debug + IntoIterator<Item = SP>,
    T: Debug + IntoIterator<Item = TP>,
    SP: AsRef<Path>,
    TP: AsRef<Path>,
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

    fn max_modified_short_circuit_on_missing<I: IntoIterator<Item = TP>, TP: AsRef<Path>>(
        paths: I,
    ) -> Option<SystemTime> {
        // Using Result and collect for short-circuit on any missing mod time
        paths
            .into_iter()
            .map(|path| max_modified(path.as_ref()).ok_or(()))
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

pub struct TaskUpToDateCheck<S, T, SP, TP, FS, FT>
where
    S: Debug + IntoIterator<Item = SP>,
    T: Debug + IntoIterator<Item = TP>,
    SP: AsRef<Path>,
    TP: AsRef<Path>,
    FS: FnOnce() -> S,
    FT: FnOnce() -> T,
{
    marker_dir: PathBuf,
    skip_check: bool,
    task_result_marker: Option<TaskResultMarker>,
    sources: FS,
    targets: FT,
}

impl<S, T, SP, TP, FS, FT> TaskUpToDateCheck<S, T, SP, TP, FS, FT>
where
    S: Debug + IntoIterator<Item = SP>,
    T: Debug + IntoIterator<Item = TP>,
    SP: AsRef<Path>,
    TP: AsRef<Path>,
    FS: FnOnce() -> S,
    FT: FnOnce() -> T,
{
    pub fn with_task_result_marker<HS: TaskResultMarkerHashSource>(
        mut self,
        source: HS,
    ) -> anyhow::Result<Self> {
        self.task_result_marker = Some(TaskResultMarker::new(&self.marker_dir, source)?);
        Ok(self)
    }

    pub fn with_sources<NS, NSP, NFS>(
        self,
        sources: NFS,
    ) -> TaskUpToDateCheck<NS, T, NSP, TP, NFS, FT>
    where
        NS: Debug + IntoIterator<Item = NSP>,
        NSP: AsRef<Path>,
        NFS: FnOnce() -> NS,
    {
        TaskUpToDateCheck {
            marker_dir: self.marker_dir,
            skip_check: self.skip_check,
            task_result_marker: self.task_result_marker,
            sources,
            targets: self.targets,
        }
    }

    pub fn with_targets<NT, NTP, NFT>(
        self,
        targets: NFT,
    ) -> TaskUpToDateCheck<S, NT, SP, NTP, FS, NFT>
    where
        NT: Debug + IntoIterator<Item = NTP>,
        NTP: AsRef<Path>,
        NFT: FnOnce() -> NT,
    {
        TaskUpToDateCheck {
            marker_dir: self.marker_dir,
            skip_check: self.skip_check,
            task_result_marker: self.task_result_marker,
            sources: self.sources,
            targets,
        }
    }

    pub fn run_or_skip<Run, Skip>(self, run: Run, skip: Skip) -> anyhow::Result<()>
    where
        Run: FnOnce() -> anyhow::Result<()>,
        Skip: FnOnce(),
    {
        if Self::is_up_to_date(
            self.task_result_marker.as_ref(),
            self.skip_check,
            self.sources,
            self.targets,
        ) {
            skip();
            Ok(())
        } else {
            match self.task_result_marker {
                Some(marker) => marker.result(run()),
                None => run(),
            }
        }
    }

    pub async fn run_async_or_skip<Run, Skip>(self, run: Run, skip: Skip) -> anyhow::Result<()>
    where
        Run: AsyncFnOnce() -> anyhow::Result<()>,
        Skip: FnOnce(),
    {
        if Self::is_up_to_date(
            self.task_result_marker.as_ref(),
            self.skip_check,
            self.sources,
            self.targets,
        ) {
            skip();
            Ok(())
        } else {
            match self.task_result_marker {
                Some(marker) => marker.result(run().await),
                None => run().await,
            }
        }
    }

    pub async fn run_async_or_skip_returning<Run, Skip, Result>(
        self,
        run: Run,
        skip: Skip,
    ) -> anyhow::Result<Option<Result>>
    where
        Run: AsyncFnOnce() -> anyhow::Result<Result>,
        Skip: FnOnce(),
    {
        if Self::is_up_to_date(
            self.task_result_marker.as_ref(),
            self.skip_check,
            self.sources,
            self.targets,
        ) {
            skip();
            Ok(None)
        } else {
            match self.task_result_marker {
                Some(marker) => Ok(Some(marker.result(run().await)?)),
                None => Ok(Some(run().await?)),
            }
        }
    }

    fn is_up_to_date(
        task_result_marker: Option<&TaskResultMarker>,
        skip_check: bool,
        sources: FS,
        targets: FT,
    ) -> bool {
        task_result_marker.is_none_or(|marker| marker.is_up_to_date())
            && is_up_to_date(skip_check, sources, targets)
    }
}

type EmptyIter = std::iter::Empty<PathBuf>;
type EmptyFn = fn() -> EmptyIter;
type EmptyTaskUpToDateCheck =
    TaskUpToDateCheck<EmptyIter, EmptyIter, PathBuf, PathBuf, EmptyFn, EmptyFn>;

impl EmptyTaskUpToDateCheck {
    pub fn empty(ctx: &BuildContext<'_>) -> Self {
        Self {
            marker_dir: ctx.application().task_result_marker_dir(),
            skip_check: ctx.skip_up_to_date_checks(),
            task_result_marker: None,
            sources: std::iter::empty::<PathBuf>,
            targets: std::iter::empty::<PathBuf>,
        }
    }
}

pub fn new_task_up_to_date_check(ctx: &BuildContext<'_>) -> EmptyTaskUpToDateCheck {
    EmptyTaskUpToDateCheck::empty(ctx)
}

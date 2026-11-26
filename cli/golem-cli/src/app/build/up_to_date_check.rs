// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use jiff::Timestamp;
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

    fn modified(path: &Path, oldest: bool) -> Option<SystemTime> {
        let mut result: Option<SystemTime> = None;
        let mut update = |modified: SystemTime| {
            if result.is_none_or(|current| {
                if oldest {
                    modified < current
                } else {
                    modified > current
                }
            }) {
                result = Some(modified)
            }
        };

        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_dir() {
                for entry in WalkDir::new(path) {
                    let metadata = entry.ok()?.metadata().ok()?;
                    if !metadata.is_dir() {
                        update(metadata.modified().ok()?);
                    }
                }
            } else if let Ok(modified) = metadata.modified() {
                update(modified)
            }
        }

        debug!(
            path = %path.display(),
            modified = result.and_then(|d| Timestamp::try_from(d).ok()).map(|d| d.to_string()),
            "modification boundary"
        );

        result
    }

    fn aggregate_modified<I: IntoIterator<Item = TP>, TP: AsRef<Path>>(
        paths: I,
        oldest: bool,
    ) -> Option<SystemTime> {
        // Using Result and collect for short-circuit on any missing mod time
        paths
            .into_iter()
            .map(|path| modified(path.as_ref(), oldest).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|mod_times| {
                if oldest {
                    mod_times.into_iter().min()
                } else {
                    mod_times.into_iter().max()
                }
                .ok_or(())
            })
            .ok()
    }

    let targets = targets();
    debug!(targets=?targets, "collected targets");

    let oldest_target_modified = aggregate_modified(targets, true);

    let oldest_target_modified = match oldest_target_modified {
        Some(modified) => modified,
        None => {
            debug!("missing targets, not up-to-date");
            return false;
        }
    };

    let sources = sources();
    debug!(source=?sources, "collected sources");

    let max_source_modified = aggregate_modified(sources, false);

    match max_source_modified {
        Some(max_source_modified) => {
            let up_to_date = max_source_modified.cmp(&oldest_target_modified) == Ordering::Less;
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

    pub fn with_output_inventory(mut self, directory: PathBuf) -> Self {
        self.task_result_marker = Some(
            self.task_result_marker
                .take()
                .expect("An output inventory requires a task marker")
                .track_outputs(directory),
        );
        self
    }

    pub fn with_skip_check(self, skip_check: bool) -> Self {
        Self {
            skip_check: self.skip_check || skip_check,
            ..self
        }
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
                Some(marker) => marker.run(run),
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
                Some(marker) => marker.run_async(run).await,
                None => run().await,
            }
        }
    }

    pub async fn run_async_or_skip_with_final_source<Run, Skip, FinalSource>(
        self,
        run: Run,
        skip: Skip,
        final_source: FinalSource,
    ) -> anyhow::Result<()>
    where
        Run: AsyncFnOnce() -> anyhow::Result<()>,
        Skip: FnOnce(),
        FinalSource: FnOnce() -> anyhow::Result<TaskResultMarker>,
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
                Some(marker) => marker.run_async_with_final_source(run, final_source).await,
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
                Some(marker) => Ok(Some(marker.run_async(run).await?)),
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
        if !task_result_marker.is_none_or(|marker| marker.is_up_to_date()) {
            return false;
        }
        match task_result_marker.and_then(TaskResultMarker::tracked_outputs) {
            Some(outputs) => is_up_to_date(skip_check, sources, || outputs),
            None => is_up_to_date(skip_check, sources, targets),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::build::task_result_marker::ExtractComponentMetadataMarkerHash;
    use crate::model::app::extracted_component_metadata_path;
    use golem_common::model::component::ComponentName;
    use test_r::test;

    #[test]
    fn every_required_target_must_be_newer_than_the_sources() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let outputs = dir.path().join("outputs");
        std::fs::create_dir(&outputs).unwrap();
        let first = outputs.join("first");
        let second = outputs.join("second");
        let write_at = |path: &Path, seconds| {
            std::fs::write(path, b"content").unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
                .unwrap();
        };
        write_at(&source, 20);
        write_at(&first, 10);
        write_at(&second, 30);
        for directory_target in [false, true] {
            let targets = || {
                if directory_target {
                    vec![&outputs]
                } else {
                    vec![&first, &second]
                }
            };
            assert!(!is_up_to_date(false, || [&source], targets));
        }
        for time in [20, 21] {
            write_at(&first, time);
            assert_eq!(
                is_up_to_date(false, || [&source], || [&first, &second]),
                time > 20
            );
            assert_eq!(is_up_to_date(false, || [&source], || [&outputs]), time > 20);
        }
        std::fs::remove_file(&first).unwrap();
        assert!(!is_up_to_date(false, || [&source], || [&first, &second]));
    }

    #[test]
    fn component_metadata_version_switch_invalidates_every_cached_output_path() {
        let dir = tempfile::tempdir().unwrap();
        let component = ComponentName("app:producer".into());
        let first_wasm = dir.path().join("first.wasm");
        let second_wasm = dir.path().join("second.wasm");
        for wasm in [&first_wasm, &second_wasm] {
            std::fs::write(wasm, b"unchanged wasm").unwrap();
            std::fs::File::options()
                .append(true)
                .open(wasm)
                .unwrap()
                .set_modified(SystemTime::now() - std::time::Duration::from_secs(86400))
                .unwrap();
        }
        let extract = |version: &str, wasm: &Path| {
            let metadata = extracted_component_metadata_path(dir.path(), &component, wasm, version);
            std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
            let mut ran = false;
            TaskUpToDateCheck {
                marker_dir: dir.path().to_path_buf(),
                skip_check: false,
                task_result_marker: None,
                sources: || vec![wasm],
                targets: || vec![&metadata],
            }
            .with_task_result_marker(ExtractComponentMetadataMarkerHash {
                component_name: &component,
            })
            .unwrap()
            .run_or_skip(
                || {
                    ran = true;
                    std::fs::write(&metadata, version)?;
                    Ok(())
                },
                || {},
            )
            .unwrap();
            (ran, std::fs::read_to_string(&metadata).unwrap())
        };

        assert_eq!(extract("1.6.0", &first_wasm), (true, "1.6.0".into()));
        assert_eq!(extract("1.6.0", &first_wasm), (false, "1.6.0".into()));
        assert_eq!(extract("1.6.1", &second_wasm), (true, "1.6.1".into()));
        assert_eq!(extract("1.6.1", &second_wasm), (false, "1.6.1".into()));
        assert_eq!(extract("1.6.1", &first_wasm), (true, "1.6.1".into()));
        assert_eq!(extract("1.6.1", &first_wasm), (false, "1.6.1".into()));
        assert_eq!(extract("1.6.0", &first_wasm), (false, "1.6.0".into()));
    }
}

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

//! The trees of the benchmark: how a tree is made, how it changes, and its hash.
//!
//! The content of a tree does not compress. After each write, the pages of the written files leave
//! the page cache, so a save reads them from the volume. The upload of a reflink capture reads its
//! files from the volume too, because a clone does not share the page cache of its source.

use anyhow::Context;
use futures::{StreamExt, TryStreamExt};
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{ConnectOptions, Connection, Executor, SqliteConnection};
use std::fs::{File, Metadata};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;

/// The name of the database file of a SQLite tree.
const DATABASE: &str = "database.sqlite";

/// The rows that one insert of a SQLite tree adds.
const ROWS_PER_INSERT: u64 = 1_000;

/// The bytes of the payload of one row of a SQLite tree.
const ROW_PAYLOAD_BYTES: u64 = 1_024;

/// The number of files that a small change rewrites, and the rows that it updates.
const CHANGED_FILES: u64 = 10;
const CHANGED_ROWS: u64 = 100;

/// The number of files in each directory of a tree at the object limit, as in the other file
/// trees.
const FILES_PER_DIRECTORY: u64 = 100;

/// A tree that the benchmark makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TreeSpec {
    pub(super) name: &'static str,
    pub(super) shape: TreeShape,
}

/// What a tree holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeShape {
    /// Files of about the same size, in directories of the same number of files.
    Files {
        files: u64,
        directories: u64,
        bytes: u64,
    },
    /// One SQLite database of at least the size.
    Sqlite { bytes: u64 },
    /// Files and directories that are `objects` filesystem objects together with the root, in
    /// the layout that [`limit_layout`] gives. Its small change keeps the number of objects.
    ObjectLimit { objects: u64, bytes: u64 },
}

pub(super) const FILES_128M: TreeSpec = TreeSpec {
    name: "files-128m",
    shape: TreeShape::Files {
        files: 10_000,
        directories: 100,
        bytes: 128 * MIB,
    },
};

pub(super) const FILES_1G: TreeSpec = TreeSpec {
    name: "files-1g",
    shape: TreeShape::Files {
        files: 10_000,
        directories: 100,
        bytes: 1024 * MIB,
    },
};

/// The tree at the object limit of an agent with 128 MiB of storage: 8,192 objects.
pub(super) const OBJECTS_128M: TreeSpec = TreeSpec {
    name: "objects-128m",
    shape: TreeShape::ObjectLimit {
        objects: 8_192,
        bytes: 128 * MIB,
    },
};

/// The tree at the object limit of an agent with 1 GiB of storage: 32,768 objects.
pub(super) const OBJECTS_1G: TreeSpec = TreeSpec {
    name: "objects-1g",
    shape: TreeShape::ObjectLimit {
        objects: 32_768,
        bytes: 1024 * MIB,
    },
};

pub(super) const SQLITE_1G: TreeSpec = TreeSpec {
    name: "sqlite-1g",
    shape: TreeShape::Sqlite { bytes: 1024 * MIB },
};

pub(super) const FILES_TINY: TreeSpec = TreeSpec {
    name: "files-tiny",
    shape: TreeShape::Files {
        files: 100,
        directories: 10,
        bytes: MIB,
    },
};

pub(super) const SQLITE_TINY: TreeSpec = TreeSpec {
    name: "sqlite-tiny",
    shape: TreeShape::Sqlite { bytes: 4 * MIB },
};

/// The number of files, directories and bytes of a tree, not counting its root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TreeCounts {
    pub(super) files: u64,
    pub(super) directories: u64,
    pub(super) bytes: u64,
}

/// Gives the number of files and the number of directories of a tree whose files, directories
/// and root are `objects` objects together.
///
/// Each directory holds at most [`FILES_PER_DIRECTORY`] files, so a directory and its files are
/// at most `FILES_PER_DIRECTORY + 1` objects.
pub(super) const fn limit_layout(objects: u64) -> (u64, u64) {
    let below_root = objects.saturating_sub(1);
    let directories = below_root.div_ceil(FILES_PER_DIRECTORY + 1);
    (below_root - directories, directories)
}

/// Makes the tree in the directory `root`, which must not exist.
pub(super) async fn generate(spec: &TreeSpec, root: &Path) -> anyhow::Result<TreeCounts> {
    std::fs::create_dir(root).with_context(|| format!("create the tree {}", root.display()))?;
    match spec.shape {
        TreeShape::Files {
            files,
            directories,
            bytes,
        } => {
            in_blocking(root, move |root| {
                generate_files(root, files, directories, bytes)
            })
            .await
        }
        TreeShape::ObjectLimit { objects, bytes } => {
            let (files, directories) = limit_layout(objects);
            in_blocking(root, move |root| {
                generate_files(root, files, directories, bytes)
            })
            .await
        }
        TreeShape::Sqlite { bytes } => generate_database(&root.join(DATABASE), bytes).await,
    }?;
    settle(root)?;
    Ok(count(root)?)
}

/// Changes a small part of the tree in `root`, and gives what changed.
pub(super) async fn change(spec: &TreeSpec, root: &Path) -> anyhow::Result<Value> {
    let change = match spec.shape {
        TreeShape::Files {
            files,
            directories,
            bytes,
        } => {
            in_blocking(root, move |root| {
                change_files(root, files, directories, bytes, Replace::No)
            })
            .await?
        }
        TreeShape::ObjectLimit { objects, bytes } => {
            let (files, directories) = limit_layout(objects);
            in_blocking(root, move |root| {
                change_files(root, files, directories, bytes, Replace::Yes)
            })
            .await?
        }
        TreeShape::Sqlite { .. } => change_database(&root.join(DATABASE)).await?,
    };
    settle(root)?;
    Ok(change)
}

/// Runs the work on the tree `root` on a blocking thread.
async fn in_blocking<T: Send + 'static>(
    root: &Path,
    work: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || work(&root)).await?
}

/// Gives the path of the file with the index, relative to the root of the tree.
fn file_path(index: u64, files: u64, directories: u64) -> Box<Path> {
    let per_directory = files.div_ceil(directories.max(1)).max(1);
    PathBuf::from(format!("d{:03}/f{index:05}", index / per_directory)).into_boxed_path()
}

/// Gives the size of the file with the index. The sizes add up to `bytes`.
fn file_size(index: u64, files: u64, bytes: u64) -> u64 {
    bytes / files.max(1) + u64::from(index < bytes % files.max(1))
}

/// Gives `size` bytes that do not compress, the same for the same path and generation.
fn content(path: &Path, generation: u8, size: u64) -> io::Result<Box<[u8]>> {
    let mut content = vec![0; usize::try_from(size).map_err(io::Error::other)?].into_boxed_slice();
    blake3::Hasher::new_derive_key("golem fs-snapshot benchmark file content")
        .update(&[generation])
        .update(path.as_os_str().as_bytes())
        .finalize_xof()
        .fill(&mut content);
    Ok(content)
}

fn write_file(root: &Path, relative: &Path, generation: u8, size: u64) -> io::Result<()> {
    let path = root.join(relative);
    std::fs::write(&path, content(relative, generation, size)?)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
}

fn generate_files(root: &Path, files: u64, directories: u64, bytes: u64) -> anyhow::Result<()> {
    (0..files).try_for_each(|index| {
        let relative = file_path(index, files, directories);
        if let Some(parent) = relative.parent() {
            let directory = root.join(parent);
            if !directory.exists() {
                std::fs::create_dir(&directory)?;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        write_file(root, &relative, 0, file_size(index, files, bytes))
    })?;
    Ok(())
}

/// Whether the small change of a files tree replaces a file, or only adds one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replace {
    /// The change deletes the last file, and adds a file with a new name in its directory. So
    /// the tree keeps its number of objects.
    Yes,
    /// The change adds a file in the first directory.
    No,
}

fn change_files(
    root: &Path,
    files: u64,
    directories: u64,
    bytes: u64,
    replace: Replace,
) -> anyhow::Result<Value> {
    let step = (files / CHANGED_FILES).max(1);
    let rewritten = (0..CHANGED_FILES.min(files))
        .map(|position| position * step)
        .try_fold(0_u64, |written, index| {
            let size = file_size(index, files, bytes);
            write_file(root, &file_path(index, files, directories), 1, size)
                .map(|()| written + size)
        })?;
    // The added file has the size of the file with the index.
    let (added_path, added_index, deleted) = match replace {
        Replace::Yes => {
            let last = files.saturating_sub(1);
            let path = file_path(last, files, directories);
            std::fs::remove_file(root.join(&path))?;
            (path.with_file_name("replaced"), last, 1)
        }
        Replace::No => (
            file_path(0, files, directories).with_file_name("added"),
            0,
            0,
        ),
    };
    let added = file_size(added_index, files, bytes);
    write_file(root, &added_path, 0, added)?;
    Ok(json!({
        "files_rewritten": CHANGED_FILES.min(files),
        "files_added": 1,
        "files_deleted": deleted,
        "rows_updated": 0,
        "bytes": rewritten + added,
    }))
}

async fn connect(path: &Path) -> anyhow::Result<SqliteConnection> {
    Ok(SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Normal)
        .page_size(4096)
        .connect()
        .await?)
}

/// Inserts rows of random payload until the database file has at least `bytes` bytes.
async fn generate_database(path: &Path, bytes: u64) -> anyhow::Result<()> {
    let mut connection = connect(path).await?;
    connection
        .execute("CREATE TABLE rows (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)")
        .await?;
    // Each row takes more than its payload, so this number of inserts is more than enough.
    let inserts = bytes / (ROWS_PER_INSERT * ROW_PAYLOAD_BYTES) + 2;
    let connection = futures::stream::iter(0..inserts)
        .map(Ok::<_, anyhow::Error>)
        .try_fold(connection, |mut connection, _| async move {
            if std::fs::metadata(path)?.len() < bytes {
                sqlx::query(
                    "WITH RECURSIVE counter(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM counter WHERE n < ?1) \
                     INSERT INTO rows (payload) SELECT randomblob(?2) FROM counter",
                )
                .bind(ROWS_PER_INSERT as i64)
                .bind(ROW_PAYLOAD_BYTES as i64)
                .execute(&mut connection)
                .await?;
            }
            Ok(connection)
        })
        .await?;
    connection.close().await?;
    Ok(())
}

/// Updates the payload of rows spread over the database.
async fn change_database(path: &Path) -> anyhow::Result<Value> {
    let mut connection = connect(path).await?;
    let last: i64 = sqlx::query_scalar("SELECT max(id) FROM rows")
        .fetch_one(&mut connection)
        .await?;
    let step = (last / CHANGED_ROWS as i64).max(1);
    let ids = (0..CHANGED_ROWS as i64)
        .map(|position| (1 + position * step).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let updated = sqlx::query(&format!(
        "UPDATE rows SET payload = randomblob({ROW_PAYLOAD_BYTES}) WHERE id IN ({ids})"
    ))
    .execute(&mut connection)
    .await?
    .rows_affected();
    connection.close().await?;
    Ok(json!({
        "files_rewritten": 0,
        "files_added": 0,
        "files_deleted": 0,
        "rows_updated": updated,
        "bytes": updated * ROW_PAYLOAD_BYTES,
    }))
}

/// How the files of a copy of a tree were made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CopyCounts {
    /// The files that share their data with the source, through a reflink.
    pub(super) reflinked: u64,
    /// The files whose bytes were copied.
    pub(super) copied: u64,
}

impl CopyCounts {
    pub(super) fn with(self, other: Self) -> Self {
        Self {
            reflinked: self.reflinked + other.reflinked,
            copied: self.copied + other.copied,
        }
    }
}

/// Copies the tree `from` into the directory `to`, which must not exist, with the permission
/// bits of each entry. A file is a reflink of its source where the filesystem has reflinks
/// (`FICLONE`, for example on XFS), and a copy of its bytes where it does not. The copy does not
/// keep the modification times.
pub(super) fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<CopyCounts> {
    std::fs::create_dir(to).with_context(|| format!("create the tree {}", to.display()))?;
    Ok(walk(
        from,
        CopyCounts::default(),
        &mut |counts, path, metadata| {
            let target = to.join(path.strip_prefix(from).map_err(io::Error::other)?);
            if metadata.is_dir() {
                std::fs::create_dir(&target)?;
                std::fs::set_permissions(&target, metadata.permissions())?;
                Ok(counts)
            } else if metadata.is_file() {
                copy_file(path, &target, metadata).map(|reflinked| {
                    counts.with(if reflinked {
                        CopyCounts {
                            reflinked: 1,
                            copied: 0,
                        }
                    } else {
                        CopyCounts {
                            reflinked: 0,
                            copied: 1,
                        }
                    })
                })
            } else {
                Err(io::Error::other(format!(
                    "the tree has an entry that is not a file or a directory: {}",
                    path.display()
                )))
            }
        },
    )?)
}

/// Copies the file `from` to the new file `to`, and tells whether the copy is a reflink.
///
/// When the reflink fails, the filesystem cannot share the data of the files, so the copy of the
/// bytes after it does not share them either.
fn copy_file(from: &Path, to: &Path, metadata: &Metadata) -> io::Result<bool> {
    let source = File::open(from)?;
    let target = File::create_new(to)?;
    match rustix::fs::ioctl_ficlone(&target, &source) {
        Ok(()) => {
            target.set_permissions(metadata.permissions())?;
            Ok(true)
        }
        Err(_) => {
            drop(target);
            std::fs::copy(from, to)?;
            Ok(false)
        }
    }
}

/// Writes each change of the filesystem of `root` to the volume, and removes the pages of each
/// file below `root` from the page cache.
pub(super) fn settle(root: &Path) -> anyhow::Result<()> {
    rustix::fs::syncfs(File::open(root)?)?;
    walk(root, (), &mut |(), path, metadata| {
        if metadata.is_file() {
            rustix::fs::fadvise(File::open(path)?, 0, None, rustix::fs::Advice::DontNeed)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn count(root: &Path) -> io::Result<TreeCounts> {
    walk(root, TreeCounts::default(), &mut |counts, _, metadata| {
        Ok(if metadata.is_dir() {
            TreeCounts {
                directories: counts.directories + 1,
                ..counts
            }
        } else if metadata.is_file() {
            TreeCounts {
                files: counts.files + 1,
                bytes: counts.bytes + metadata.len(),
                ..counts
            }
        } else {
            counts
        })
    })
}

/// Gives the hash of the tree below `root` and its counts, and reads the tree on a blocking thread.
pub(super) async fn hash(root: &Path) -> anyhow::Result<(Box<str>, TreeCounts)> {
    let root = root.to_path_buf();
    Ok(tokio::task::spawn_blocking(move || tree_hash(&root)).await??)
}

/// Gives the hash of the tree below `root` and its counts.
///
/// The hash covers the path, the kind, the permission bits and the modification time of each
/// entry, the size and the content of each file, and the target of each symlink. It does not
/// cover the owner or the metadata of `root`.
pub(super) fn tree_hash(root: &Path) -> io::Result<(Box<str>, TreeCounts)> {
    let hasher = walk(
        root,
        blake3::Hasher::new(),
        &mut |mut hasher, path, metadata| {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            let name = relative.as_os_str().as_bytes();
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name);
            hasher.update(&metadata.mode().to_le_bytes());
            hasher.update(&metadata.mtime().to_le_bytes());
            hasher.update(&metadata.mtime_nsec().to_le_bytes());
            if metadata.is_file() {
                hasher.update(b"f");
                hasher.update(&metadata.len().to_le_bytes());
                let mut content = blake3::Hasher::new();
                content.update_reader(File::open(path)?)?;
                hasher.update(content.finalize().as_bytes());
            } else if metadata.is_symlink() {
                let target = std::fs::read_link(path)?;
                hasher.update(b"l");
                hasher.update(&(target.as_os_str().len() as u64).to_le_bytes());
                hasher.update(target.as_os_str().as_bytes());
            } else if metadata.is_dir() {
                hasher.update(b"d");
            } else {
                hasher.update(b"o");
            }
            Ok(hasher)
        },
    )?;
    Ok((hasher.finalize().to_hex().as_str().into(), count(root)?))
}

/// Visits each entry below `directory`, in the order of the names, parents before children, and
/// folds the visits into one value. Symlinks are not followed.
fn walk<T>(
    directory: &Path,
    initial: T,
    visit: &mut impl FnMut(T, &Path, &Metadata) -> io::Result<T>,
) -> io::Result<T> {
    let mut names = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    names.into_iter().try_fold(initial, |value, name| {
        let path = directory.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        let value = visit(value, &path, &metadata)?;
        if metadata.is_dir() {
            walk(&path, value, visit)
        } else {
            Ok(value)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FILES_TINY, MIB, SQLITE_TINY, TreeCounts, TreeShape, TreeSpec, change, copy_tree,
        file_path, file_size, generate, limit_layout, tree_hash, walk,
    };
    use pretty_assertions::assert_eq;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};
    use test_r::test;

    /// A tree at the object limit of 128 objects: 125 files in 2 directories, and the root.
    const OBJECTS_TINY: TreeSpec = TreeSpec {
        name: "objects-tiny",
        shape: TreeShape::ObjectLimit {
            objects: 128,
            bytes: MIB,
        },
    };

    fn hash(root: &Path) -> Box<str> {
        tree_hash(root).unwrap().0
    }

    /// Gives the path, the permission bits and the content of each entry below `root`, in the
    /// order of the paths. A directory has no content.
    fn contents(root: &Path) -> Vec<(PathBuf, u32, Vec<u8>)> {
        walk(root, Vec::new(), &mut |mut entries, path, metadata| {
            let content = if metadata.is_file() {
                std::fs::read(path)?
            } else {
                Vec::new()
            };
            entries.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                metadata.mode(),
                content,
            ));
            Ok(entries)
        })
        .unwrap()
    }

    #[test]
    fn the_limit_layout_counts_the_root_and_fills_each_directory() {
        let layout = |objects| {
            let (files, directories) = limit_layout(objects);
            (
                files,
                directories,
                files + directories + 1,
                file_path(files - 1, files, directories),
            )
        };

        assert_eq!(
            [layout(8_192), layout(32_768), layout(128)],
            [
                (8_109, 82, 8_192, Path::new("d081/f08108").into()),
                (32_442, 325, 32_768, Path::new("d324/f32441").into()),
                (125, 2, 128, Path::new("d001/f00124").into()),
            ]
        );
    }

    #[test]
    async fn a_tree_at_the_object_limit_keeps_its_number_of_objects_after_its_change() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("tree");

        let counts = generate(&OBJECTS_TINY, &root).await.unwrap();
        let before = hash(&root);
        let changed = change(&OBJECTS_TINY, &root).await.unwrap();
        let (after, after_counts) = tree_hash(&root).unwrap();

        assert_eq!(
            (
                counts,
                after_counts,
                [
                    &changed["files_rewritten"],
                    &changed["files_added"],
                    &changed["files_deleted"]
                ]
                .map(|value| value.as_u64()),
                root.join("d001/f00124").exists(),
                root.join("d001/replaced").exists(),
                before != after,
            ),
            (
                TreeCounts {
                    files: 125,
                    directories: 2,
                    bytes: MIB,
                },
                TreeCounts {
                    files: 125,
                    directories: 2,
                    bytes: MIB,
                },
                [Some(10), Some(1), Some(1)],
                false,
                true,
                true,
            )
        );
    }

    #[test]
    async fn a_copy_of_a_tree_has_its_entries_permissions_and_content() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("tree");
        let copy = work.path().join("copy");
        generate(&FILES_TINY, &root).await.unwrap();
        std::fs::set_permissions(
            root.join("d000/f00000"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        std::fs::set_permissions(root.join("d001"), std::fs::Permissions::from_mode(0o700))
            .unwrap();

        let counts = copy_tree(&root, &copy).unwrap();

        assert_eq!(
            (counts.reflinked + counts.copied, contents(&copy)),
            (100, contents(&root))
        );
    }

    #[test]
    fn the_file_sizes_add_up_and_the_files_fill_the_directories() {
        assert_eq!(
            (
                (0..7).map(|index| file_size(index, 7, 100)).sum::<u64>(),
                (0..7)
                    .map(|index| file_size(index, 7, 100))
                    .collect::<Vec<_>>(),
                file_path(0, 100, 10),
                file_path(19, 100, 10),
                file_path(99, 100, 10),
            ),
            (
                100,
                vec![15, 15, 14, 14, 14, 14, 14],
                Path::new("d000/f00000").into(),
                Path::new("d001/f00019").into(),
                Path::new("d009/f00099").into(),
            )
        );
    }

    #[test]
    async fn a_files_tree_has_its_files_directories_and_bytes_and_a_change_changes_it() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("tree");

        let counts = generate(&FILES_TINY, &root).await.unwrap();
        let before = hash(&root);
        let changed = change(&FILES_TINY, &root).await.unwrap();
        let (after, after_counts) = tree_hash(&root).unwrap();

        assert_eq!(
            (
                counts,
                changed["files_rewritten"].as_u64(),
                changed["files_added"].as_u64(),
                changed["files_deleted"].as_u64(),
                after_counts.files,
                before != after,
            ),
            (
                TreeCounts {
                    files: 100,
                    directories: 10,
                    bytes: 1024 * 1024,
                },
                Some(10),
                Some(1),
                Some(0),
                101,
                true,
            )
        );
    }

    #[test]
    async fn a_sqlite_tree_has_one_database_of_at_least_its_size() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("tree");

        let counts = generate(&SQLITE_TINY, &root).await.unwrap();
        let before = hash(&root);
        let changed = change(&SQLITE_TINY, &root).await.unwrap();

        assert_eq!(
            (
                counts.files,
                counts.bytes >= 4 * 1024 * 1024,
                counts.bytes < 6 * 1024 * 1024,
                changed["rows_updated"].as_u64(),
                before != hash(&root),
            ),
            (1, true, true, Some(100), true)
        );
    }

    #[test]
    fn the_tree_hash_changes_with_each_kept_attribute() {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        let time = SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 5);
        let reset = |root: &Path| {
            let _ = std::fs::remove_dir_all(root.join("dir"));
            let _ = std::fs::remove_file(root.join("file"));
            let _ = std::fs::remove_file(root.join("other"));
            let _ = std::fs::remove_file(root.join("link"));
            std::fs::create_dir(root.join("dir")).unwrap();
            std::fs::write(root.join("file"), b"content").unwrap();
            std::fs::set_permissions(root.join("file"), std::fs::Permissions::from_mode(0o644))
                .unwrap();
            std::os::unix::fs::symlink("file", root.join("link")).unwrap();
            std::fs::File::options()
                .write(true)
                .open(root.join("file"))
                .unwrap()
                .set_modified(time)
                .unwrap();
            fs_set_times::set_times(
                root.join("dir"),
                None,
                Some(fs_set_times::SystemTimeSpec::Absolute(time)),
            )
            .unwrap();
            fs_set_times::set_symlink_times(
                root.join("link"),
                None,
                Some(fs_set_times::SystemTimeSpec::Absolute(time)),
            )
            .unwrap();
        };
        let changed = |change: &dyn Fn(&Path)| {
            reset(root);
            change(root);
            hash(root)
        };
        reset(root);
        let original = hash(root);

        let hashes = [
            changed(&|_| {}),
            changed(&|root| {
                std::fs::write(root.join("file"), b"CONTENT").unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(root.join("file"))
                    .unwrap()
                    .set_modified(time)
                    .unwrap();
            }),
            changed(&|root| {
                std::fs::set_permissions(root.join("file"), std::fs::Permissions::from_mode(0o600))
                    .unwrap()
            }),
            changed(&|root| {
                std::fs::File::options()
                    .write(true)
                    .open(root.join("file"))
                    .unwrap()
                    .set_modified(time + Duration::from_nanos(1))
                    .unwrap()
            }),
            changed(&|root| {
                std::fs::remove_file(root.join("link")).unwrap();
                std::os::unix::fs::symlink("elsewhere", root.join("link")).unwrap();
                fs_set_times::set_symlink_times(
                    root.join("link"),
                    None,
                    Some(fs_set_times::SystemTimeSpec::Absolute(time)),
                )
                .unwrap();
            }),
            changed(&|root| std::fs::rename(root.join("file"), root.join("other")).unwrap()),
            changed(&|root| {
                std::fs::create_dir(root.join("dir/empty")).unwrap();
                [root.join("dir/empty"), root.join("dir")]
                    .iter()
                    .for_each(|directory| {
                        fs_set_times::set_times(
                            directory,
                            None,
                            Some(fs_set_times::SystemTimeSpec::Absolute(time)),
                        )
                        .unwrap()
                    });
            }),
        ];

        assert_eq!(
            hashes
                .iter()
                .map(|hash| *hash == original)
                .collect::<Vec<_>>(),
            vec![true, false, false, false, false, false, false]
        );
    }
}

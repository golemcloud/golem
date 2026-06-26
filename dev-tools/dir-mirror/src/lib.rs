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

//! Idempotent directory mirror: make a destination directory a byte-identical copy of a
//! source directory, rewriting only files whose bytes changed (so unchanged files keep
//! their mtime) and deleting files not present in the source.
//!
//! Preserving mtimes is the point: the `wit` cargo-make tasks use this to sync the
//! per-crate `wit/deps` copies, which cargo tracks via `rerun-if-changed`. A plain
//! recopy would bump every mtime and rebuild the workspace on every build.
//!
//! Only regular files are mirrored — symlinks and empty directories in `src` are not
//! reproduced (fine for `wit/deps`, which is plain `.wit` files).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
test_r::enable!();

pub const USAGE: &str = "Usage: dir-mirror <src1> <dst1> [<src2> <dst2> ...]\n\
     \n\
     Makes each <dst> a byte-identical copy of <src>: unchanged files are left\n\
     untouched (preserving their mtime), and files/dirs in <dst> absent from\n\
     <src> are removed.";

/// Mirror each `<src> <dst>` pair in `args` (a flat list). Errors on an empty or odd-length
/// argument list. Each pair's error is annotated with the `src -> dst` it came from.
pub fn run(args: &[String]) -> io::Result<()> {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE));
    }
    for pair in args.chunks(2) {
        let (src, dst) = (Path::new(&pair[0]), Path::new(&pair[1]));
        mirror(src, dst)
            .map_err(|e| io::Error::new(e.kind(), format!("{} -> {}: {e}", pair[0], pair[1])))?;
    }
    Ok(())
}

/// Make `dst` a byte-identical copy of the directory `src`, preserving the mtime of
/// files that are already up to date and removing anything in `dst` not in `src`.
pub fn mirror(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination path is empty",
        ));
    }
    if !src.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("source directory does not exist: {}", src.display()),
        ));
    }

    let want: BTreeSet<PathBuf> = walk_files(src)?
        .into_iter()
        .map(|f| f.strip_prefix(src).expect("walked under src").to_path_buf())
        .collect();

    for rel in &want {
        copy_if_different(&src.join(rel), &dst.join(rel))?;
    }
    for file in walk_files(dst)? {
        if !want.contains(file.strip_prefix(dst).expect("walked under dst")) {
            fs::remove_file(&file)?;
        }
    }
    remove_empty_dirs(dst)
}

/// Copy `src` to `dst` only when `dst` is missing or differs, creating parent dirs as
/// needed. Returns whether a write happened.
fn copy_if_different(src: &Path, dst: &Path) -> io::Result<bool> {
    let new = fs::read(src)?;
    if let Ok(old) = fs::read(dst)
        && old == new
    {
        return Ok(false);
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dst, &new)?;
    Ok(true)
}

/// Recursively collect all regular files under `dir` (empty if `dir` is absent).
fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

/// Remove empty directories under `dir`, deepest first so a parent is reconsidered
/// after its children are gone. `dir` itself is kept.
fn remove_empty_dirs(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut dirs = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let path = entry.path();
                dirs.push(path.clone());
                stack.push(path);
            }
        }
    }
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for d in dirs {
        if fs::read_dir(&d)?.next().is_none() {
            fs::remove_dir(&d)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use test_r::test;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn copies_exact_tree_from_empty() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        write(&src.join("a.txt"), "a");
        write(&src.join("nested/b.txt"), "b");

        mirror(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dst.join("nested/b.txt")).unwrap(), "b");
    }

    #[test]
    fn does_not_rewrite_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src/x.txt"), tmp.path().join("dst/x.txt"));
        write(&src, "same");

        assert!(copy_if_different(&src, &dst).unwrap(), "first copy writes");
        assert!(
            !copy_if_different(&src, &dst).unwrap(),
            "identical content is not rewritten"
        );
    }

    #[test]
    fn preserves_mtime_of_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        write(&src.join("f.txt"), "x");
        mirror(&src, &dst).unwrap();
        let before = fs::metadata(dst.join("f.txt")).unwrap().modified().unwrap();

        // Sleep so a (wrong) rewrite would land on a later mtime; an unchanged file keeps
        // the exact same timestamp regardless.
        std::thread::sleep(std::time::Duration::from_millis(20));
        mirror(&src, &dst).unwrap();
        let after = fs::metadata(dst.join("f.txt")).unwrap().modified().unwrap();

        assert_eq!(before, after, "unchanged file must keep its mtime");
    }

    #[test]
    fn propagates_changed_file() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        write(&src.join("f.txt"), "v1");
        mirror(&src, &dst).unwrap();

        write(&src.join("f.txt"), "v2");
        mirror(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("f.txt")).unwrap(), "v2");
    }

    #[test]
    fn prunes_files_absent_from_source() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        write(&src.join("keep.txt"), "k");
        write(&dst.join("keep.txt"), "k");
        write(&dst.join("stale.txt"), "old");

        mirror(&src, &dst).unwrap();

        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join("stale.txt").exists());
    }

    #[test]
    fn removes_emptied_directories() {
        let tmp = TempDir::new().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        write(&src.join("keep.txt"), "k");
        write(&dst.join("keep.txt"), "k");
        write(&dst.join("gone/deep/old.txt"), "old");

        mirror(&src, &dst).unwrap();

        assert!(!dst.join("gone").exists());
    }

    #[test]
    fn errors_when_source_missing() {
        let tmp = TempDir::new().unwrap();
        let err = mirror(&tmp.path().join("missing"), &tmp.path().join("dst")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn errors_on_empty_destination() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        write(&src.join("f.txt"), "x");
        let err = mirror(&src, Path::new("")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn run_rejects_odd_argument_count() {
        let err = run(&["only-one".to_string()]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(run(&[]).is_err(), "no arguments is rejected");
    }

    #[test]
    fn run_mirrors_multiple_pairs() {
        let tmp = TempDir::new().unwrap();
        let (src_a, dst_a) = (tmp.path().join("src_a"), tmp.path().join("dst_a"));
        let (src_b, dst_b) = (tmp.path().join("src_b"), tmp.path().join("dst_b"));
        write(&src_a.join("a.txt"), "a");
        write(&src_b.join("b.txt"), "b");

        let args = [
            src_a.to_str().unwrap().to_string(),
            dst_a.to_str().unwrap().to_string(),
            src_b.to_str().unwrap().to_string(),
            dst_b.to_str().unwrap().to_string(),
        ];
        run(&args).unwrap();

        assert_eq!(fs::read_to_string(dst_a.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dst_b.join("b.txt")).unwrap(), "b");
    }
}

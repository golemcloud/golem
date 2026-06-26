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

pub const USAGE: &str = "Usage: dir-mirror --src <dir> --dst <dir> [--src <dir> --dst <dir> ...]\n\
     \n\
     Makes each <dst> a byte-identical copy of <src>: unchanged files are left\n\
     untouched (preserving their mtime), and files/dirs in <dst> absent from\n\
     <src> are removed.";

/// Parse one or more `--src <dir> --dst <dir>` pairs. The flags are required (and must
/// alternate, starting with `--src`) so a source and destination can't be silently
/// swapped or misaligned.
fn parse_pairs(args: &[String]) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let usage_err =
        |msg: String| io::Error::new(io::ErrorKind::InvalidInput, format!("{msg}\n\n{USAGE}"));
    // A flag value must be present and not itself look like a flag.
    let value = |at: usize, flag: &str| match args.get(at) {
        Some(v) if !v.starts_with("--") => Ok(PathBuf::from(v)),
        _ => Err(usage_err(format!("{flag} requires a directory value"))),
    };

    let mut pairs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] != "--src" {
            return Err(usage_err(format!("expected --src, found {:?}", args[i])));
        }
        let src = value(i + 1, "--src")?;
        if args.get(i + 2).map(String::as_str) != Some("--dst") {
            return Err(usage_err(format!(
                "--src {} must be followed by --dst",
                src.display()
            )));
        }
        let dst = value(i + 3, "--dst")?;
        pairs.push((src, dst));
        i += 4;
    }
    if pairs.is_empty() {
        return Err(usage_err(
            "expected at least one --src <dir> --dst <dir> pair".into(),
        ));
    }
    Ok(pairs)
}

/// Mirror each `--src <dir> --dst <dir>` pair given in `args`. Each pair's error is
/// annotated with the `src -> dst` it came from.
pub fn run(args: &[String]) -> io::Result<()> {
    for (src, dst) in parse_pairs(args)? {
        mirror(&src, &dst).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("{} -> {}: {e}", src.display(), dst.display()),
            )
        })?;
    }
    Ok(())
}

/// Make `dst` a byte-identical copy of the directory `src`, preserving the mtime of
/// files that are already up to date and removing anything in `dst` not in `src`.
pub fn mirror(src: &Path, dst: &Path) -> io::Result<()> {
    if is_unsafe_dst(src, dst) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to mirror into unsafe destination: {dst:?}"),
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
            fs::remove_file(&file).map_err(|e| with_path(e, "removing", &file))?;
        }
    }
    remove_empty_dirs(dst)
}

/// `mirror` prunes `dst` (deletes files in it but not in `src`), so reject destinations
/// whose prune could reach unrelated files:
/// - empty, the current directory (`.`), or the filesystem root;
/// - any path containing a `..` component, which can escape the intended subtree;
/// - a path that overlaps `src` (one is an ancestor of the other), unless they're equal.
fn is_unsafe_dst(src: &Path, dst: &Path) -> bool {
    use std::path::Component;
    dst.as_os_str().is_empty()
        || dst == Path::new(".")
        || dst.parent().is_none()
        || dst.components().any(|c| c == Component::ParentDir)
        || (src != dst && (src.starts_with(dst) || dst.starts_with(src)))
}

/// Annotate an io error with the path it happened on (std fs errors don't embed it).
fn with_path(e: io::Error, action: &str, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{action} {}: {e}", path.display()))
}

/// Copy `src` to `dst` only when `dst` is missing or differs, creating parent dirs as
/// needed. Returns whether a write happened.
fn copy_if_different(src: &Path, dst: &Path) -> io::Result<bool> {
    let new = fs::read(src).map_err(|e| with_path(e, "reading", src))?;
    if let Ok(old) = fs::read(dst)
        && old == new
    {
        return Ok(false);
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| with_path(e, "creating", parent))?;
    }
    fs::write(dst, &new).map_err(|e| with_path(e, "writing", dst))?;
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
    fn refuses_dangerous_destinations() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("a/b");
        write(&src.join("f.txt"), "x");

        for bad in [
            Path::new(""),
            Path::new("."),
            Path::new(".."),
            Path::new("/"),
        ] {
            let err = mirror(&src, bad).unwrap_err();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidInput,
                "destination {bad:?} must be refused"
            );
        }
        // A destination that overlaps the source (ancestor or descendant), or escapes via
        // `..`, would prune unrelated files.
        let overlapping = [
            tmp.path().join("a"),        // ancestor of src
            src.join("inner"),           // descendant of src
            tmp.path().join("a/b/../c"), // ".." escape component
        ];
        for bad in &overlapping {
            let err = mirror(&src, bad).unwrap_err();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidInput,
                "destination {bad:?} must be refused"
            );
        }
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn run_rejects_malformed_args() {
        let cases: &[&[&str]] = &[
            &[],                           // nothing
            &["src", "dst"],               // bare positional, no flags
            &["--src", "x"],               // missing --dst
            &["--src", "x", "--dst"],      // --dst without a value
            &["--src", "--dst", "y"],      // --src value missing (looks like a flag)
            &["--dst", "y", "--src", "x"], // wrong order
        ];
        for case in cases {
            let args: Vec<String> = case.iter().map(|a| s(a)).collect();
            let err = run(&args).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "case {case:?}");
        }
    }

    #[test]
    fn run_mirrors_multiple_flagged_pairs() {
        let tmp = TempDir::new().unwrap();
        let (src_a, dst_a) = (tmp.path().join("src_a"), tmp.path().join("dst_a"));
        let (src_b, dst_b) = (tmp.path().join("src_b"), tmp.path().join("dst_b"));
        write(&src_a.join("a.txt"), "a");
        write(&src_b.join("b.txt"), "b");

        let args = [
            s("--src"),
            src_a.to_str().unwrap().to_string(),
            s("--dst"),
            dst_a.to_str().unwrap().to_string(),
            s("--src"),
            src_b.to_str().unwrap().to_string(),
            s("--dst"),
            dst_b.to_str().unwrap().to_string(),
        ];
        run(&args).unwrap();

        assert_eq!(fs::read_to_string(dst_a.join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dst_b.join("b.txt")).unwrap(), "b");
    }
}

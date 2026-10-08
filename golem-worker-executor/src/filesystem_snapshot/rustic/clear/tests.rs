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

use super::{ClearEntry, clear_entry};
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
fn the_clear_enters_only_a_directory_of_the_same_device_and_unlinks_each_other_entry() {
    assert_eq!(
        [
            clear_entry(true, true),
            clear_entry(true, false),
            clear_entry(false, true),
            clear_entry(false, false),
        ],
        [
            ClearEntry::ChmodThenEnter,
            ClearEntry::OtherDevice,
            ClearEntry::Unlink,
            ClearEntry::Unlink,
        ]
    );
}

#[cfg(unix)]
mod unix {
    use super::super::{ClearHook, MOST_CLEAR_DEPTH, clear, clear_with, is_empty};
    use pretty_assertions::assert_eq;
    use std::ffi::OsStr;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use test_r::test;

    /// The entries below `root`, each with its kind, its mode, and the content of a file, in the
    /// order of the paths.
    fn listing(root: &Path) -> Vec<(PathBuf, &'static str, u32, Vec<u8>)> {
        fn walk(
            root: &Path,
            directory: &Path,
            listed: &mut Vec<(PathBuf, &'static str, u32, Vec<u8>)>,
        ) {
            let mut entries = std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            entries.sort();
            entries.into_iter().for_each(|path| {
                let metadata = std::fs::symlink_metadata(&path).unwrap();
                let mode = metadata.permissions().mode() & 0o7777;
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                if metadata.is_dir() {
                    listed.push((relative, "dir", mode, Vec::new()));
                    walk(root, &path, listed);
                } else if metadata.file_type().is_symlink() {
                    listed.push((relative, "link", 0, Vec::new()));
                } else {
                    listed.push((relative, "file", mode, std::fs::read(&path).unwrap()));
                }
            });
        }
        let mut listed = Vec::new();
        walk(root, root, &mut listed);
        listed
    }

    /// Gives a parent directory with `into` and its sibling `outside`, which holds a file and a
    /// directory at mode 0555 with a file in it.
    fn into_and_outside() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let into = parent.path().join("into");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&into).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("file"), b"outside").unwrap();
        std::fs::create_dir(outside.join("locked")).unwrap();
        std::fs::write(outside.join("locked/kept"), b"kept").unwrap();
        std::fs::set_permissions(
            outside.join("locked"),
            std::fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        (parent, into, outside)
    }

    /// Gives back the owner permissions of each directory below `root`, so a temporary directory
    /// can go.
    fn unlock(root: &Path) {
        if let Ok(entries) = std::fs::read_dir(root) {
            entries.flatten().for_each(|entry| {
                let path = entry.path();
                if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                    unlock(&path);
                }
            });
        }
    }

    #[test]
    fn the_clear_of_into_never_follows_a_link() {
        let (parent, into, outside) = into_and_outside();
        let before = listing(&outside);
        symlink("/", into.join("x")).unwrap();
        symlink("../outside", into.join("y")).unwrap();
        symlink("../outside/file", into.join("z")).unwrap();
        std::fs::create_dir(into.join("d")).unwrap();
        symlink("../../outside", into.join("d/w")).unwrap();
        std::fs::write(into.join("d/file"), b"inside").unwrap();

        let cleared = clear(&into).map_err(|error| error.to_string());

        assert_eq!(
            (cleared, is_empty(&into).unwrap(), listing(&outside)),
            (Ok(()), true, before)
        );
        unlock(parent.path());
    }

    #[test]
    fn the_clear_removes_directories_at_modes_0555_0444_and_0000() {
        let parent = tempfile::tempdir().unwrap();
        let into = parent.path().join("into");
        std::fs::create_dir(&into).unwrap();
        let modes = if cfg!(target_os = "linux") {
            vec![0o555, 0o444, 0o000]
        } else {
            vec![0o555, 0o444]
        };
        modes.iter().for_each(|mode| {
            let directory = into.join(format!("d{mode:o}"));
            std::fs::create_dir(&directory).unwrap();
            std::fs::create_dir(directory.join("below")).unwrap();
            std::fs::write(directory.join("below/file"), b"file").unwrap();
            std::fs::set_permissions(
                directory.join("below"),
                std::fs::Permissions::from_mode(*mode),
            )
            .unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(*mode)).unwrap();
        });
        // A process that may read and write any directory lists each of them, and then the modes
        // test nothing. The case holds only when the process cannot list a directory at 0o444
        // below one at 0o555.
        let unlistable = std::fs::read_dir(into.join("d444/below")).is_err();
        let privileged = std::fs::write(into.join("d555/probe"), b"probe").is_ok();

        let cleared = clear(&into).map_err(|error| error.to_string());

        assert_eq!(
            (cleared, is_empty(&into).unwrap(), unlistable || privileged),
            (Ok(()), true, true)
        );
        if privileged {
            eprintln!("the process may write any directory, so the modes test nothing here");
        }
        unlock(parent.path());
    }

    #[test]
    fn the_clear_refuses_a_tree_deeper_than_the_limit() {
        let parent = tempfile::tempdir().unwrap();
        let into = parent.path().join("into");
        let deepest = (0..=MOST_CLEAR_DEPTH).fold(into.clone(), |path, _| path.join("d"));
        std::fs::create_dir_all(&deepest).unwrap();
        let shallow = tempfile::tempdir().unwrap();
        let limit =
            (0..MOST_CLEAR_DEPTH).fold(shallow.path().to_path_buf(), |path, _| path.join("d"));
        std::fs::create_dir_all(&limit).unwrap();

        let deep = clear(&into).map_err(|error| error.to_string());
        let at_the_limit = clear(shallow.path()).map_err(|error| error.to_string());

        assert_eq!(
            (deep, at_the_limit, is_empty(shallow.path()).unwrap()),
            (
                Err(format!(
                    "the tree of the restore is deeper than {MOST_CLEAR_DEPTH} levels"
                )),
                Ok(()),
                true
            )
        );
    }

    /// A hook that swaps the directory `name` of `into` for a link to `target` at one point of the
    /// walk, and moves the directory to `moved`.
    struct Swap {
        name: &'static str,
        target: PathBuf,
        moved: PathBuf,
        at_mode: bool,
        done: Mutex<bool>,
    }

    impl Swap {
        fn swap(&self, parent: &Path, name: &OsStr) {
            let mut done = self.done.lock().unwrap();
            if !*done && name == self.name {
                std::fs::rename(parent.join(name), &self.moved).unwrap();
                symlink(&self.target, parent.join(name)).unwrap();
                *done = true;
            }
        }
    }

    impl ClearHook for Swap {
        fn before_open(&self, parent: &Path, name: &OsStr) {
            if !self.at_mode {
                self.swap(parent, name);
            }
        }

        fn before_mode(&self, parent: &Path, name: &OsStr) {
            if self.at_mode {
                self.swap(parent, name);
            }
        }
    }

    #[test]
    fn a_directory_swapped_for_a_link_before_its_open_fails_the_clear_and_changes_nothing_outside()
    {
        let (parent, into, outside) = into_and_outside();
        std::fs::create_dir(into.join("d")).unwrap();
        let before = listing(&outside);
        let swap = Swap {
            name: "d",
            target: outside.join("locked"),
            moved: parent.path().join("moved"),
            at_mode: false,
            done: Mutex::new(false),
        };

        let cleared = clear_with(&into, &swap);

        assert_eq!((cleared.is_err(), listing(&outside)), (true, before));
        unlock(parent.path());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_directory_swapped_after_its_open_gets_the_mode_and_its_link_target_does_not() {
        let (parent, into, outside) = into_and_outside();
        // A directory moves to another parent only with write permission on it.
        std::fs::create_dir(into.join("d")).unwrap();
        std::fs::set_permissions(into.join("d"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let moved = parent.path().join("moved");
        let swap = Swap {
            name: "d",
            target: outside.join("locked"),
            moved: moved.clone(),
            at_mode: true,
            done: Mutex::new(false),
        };
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;

        let cleared = clear_with(&into, &swap);

        assert_eq!(
            (
                cleared.is_err(),
                mode(&outside.join("locked")),
                mode(&moved),
                std::fs::read(outside.join("locked/kept")).unwrap()
            ),
            (true, 0o555, 0o700, b"kept".to_vec())
        );
        unlock(parent.path());
    }
}

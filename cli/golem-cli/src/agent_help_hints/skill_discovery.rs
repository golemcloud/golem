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

//! Locating skills installed under the active Golem application.

use std::path::{Path, PathBuf};

/// Returns the absolute path of `<app_dir>/.agents/skills/` if and only if:
///   - the current working directory is inside (or above) a Golem application
///     manifest discoverable via the same walk used by the rest of the CLI,
///   - that application directory contains an `.agents/skills/` directory.
///
/// Returns `None` otherwise. Does no I/O beyond the manifest walk and a
/// single `is_dir` check.
pub fn find_app_skill_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_app_skill_root_from(&cwd)
}

pub fn find_app_skill_root_from(start_dir: &Path) -> Option<PathBuf> {
    let main_source = crate::app::context::find_main_source_from(start_dir)?;
    let app_dir = main_source.parent()?;
    let skill_root = app_dir.join(".agents").join("skills");
    if skill_root.is_dir() {
        Some(skill_root)
    } else {
        None
    }
}

/// True if `<skill_root>/<name>/SKILL.md` exists as a file.
pub fn skill_is_installed(skill_root: &Path, name: &str) -> bool {
    skill_root.join(name).join("SKILL.md").is_file()
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;
    use test_r::test;

    #[test]
    fn skill_is_installed_detects_existing_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skill = root.join("my-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: my-skill\n---\n").unwrap();

        assert!(skill_is_installed(root, "my-skill"));
        assert!(!skill_is_installed(root, "no-such-skill"));
    }

    #[test]
    fn skill_is_installed_requires_skill_md_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skill = root.join("dir-only-skill");
        fs::create_dir_all(&skill).unwrap();
        // No SKILL.md file inside.

        assert!(!skill_is_installed(root, "dir-only-skill"));
    }
}

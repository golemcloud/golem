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

//! The prompt's history: one file per agent, kept across sessions.

use super::contract::local_command;
use reedline::{
    FileBackedHistory, History, HistoryItem, HistoryItemId, HistorySessionId, SearchQuery,
};
use std::path::{Path, PathBuf};

/// The number of commands kept per agent.
const CAPACITY: usize = 2000;

/// The history file of one agent on one server: `<config>/ssh-history/<agent>-<hash>.txt`.
/// The hash covers every part, so two agents never share a file.
pub fn history_file(
    config_dir: &Path,
    server: &str,
    application: &str,
    environment: &str,
    component: &str,
    agent: &str,
) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    for part in [server, application, environment, component, agent] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    let hash = hasher.finalize().to_hex();
    let name: String = agent
        .chars()
        .map(|character| match character {
            character if character.is_ascii_alphanumeric() => character,
            '.' | '_' | '-' => character,
            _ => '_',
        })
        .take(40)
        .collect();
    config_dir
        .join("ssh-history")
        .join(format!("{name}-{}.txt", &hash[..16]))
}

/// Whether a submitted line is kept: not one that starts with a space, as in bash, and not one
/// the session handles itself.
pub fn is_recorded(line: &str) -> bool {
    !line.starts_with(' ') && local_command(line).is_none()
}

/// History that skips the lines [`is_recorded`] rejects.
pub struct SessionHistory {
    inner: FileBackedHistory,
}

impl SessionHistory {
    /// Opens the history kept in `path`, for its owner only: the file and its directory are
    /// created that way, and closed to others when they already exist and are not.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(directory) = path.parent() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(directory)?;
            #[cfg(unix)]
            owner_only(directory, 0o700)?;
        }
        // A link here would send every command typed to whatever file it points at.
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(std::io::Error::other("it is a symbolic link"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path)?;
        #[cfg(unix)]
        owner_only(path, 0o600)?;
        let inner = FileBackedHistory::with_file(CAPACITY, path.to_path_buf())
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self { inner })
    }

    /// History for this session only.
    pub fn in_memory() -> Self {
        Self {
            inner: FileBackedHistory::new(CAPACITY).expect("the capacity is below the maximum"),
        }
    }
}

/// Takes away what group and others may do with `path`, when they may do anything.
#[cfg(unix)]
fn owner_only(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

impl History for SessionHistory {
    fn save(&mut self, item: HistoryItem) -> reedline::Result<HistoryItem> {
        if is_recorded(&item.command_line) {
            self.inner.save(item)
        } else {
            Ok(item)
        }
    }

    fn load(&self, id: HistoryItemId) -> reedline::Result<HistoryItem> {
        self.inner.load(id)
    }

    fn count(&self, query: SearchQuery) -> reedline::Result<i64> {
        self.inner.count(query)
    }

    fn search(&self, query: SearchQuery) -> reedline::Result<Vec<HistoryItem>> {
        self.inner.search(query)
    }

    fn update(
        &mut self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> reedline::Result<()> {
        self.inner.update(id, updater)
    }

    fn clear(&mut self) -> reedline::Result<()> {
        self.inner.clear()
    }

    fn delete(&mut self, id: HistoryItemId) -> reedline::Result<()> {
        self.inner.delete(id)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn session(&self) -> Option<HistorySessionId> {
        self.inner.session()
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionHistory, history_file, is_recorded};
    use reedline::{History, HistoryItem, SearchDirection, SearchQuery};
    use std::path::Path;
    use test_r::test;

    fn file(server: &str, environment: &str, agent: &str) -> String {
        history_file(
            Path::new("/cfg"),
            server,
            "app",
            environment,
            "component-1",
            agent,
        )
        .display()
        .to_string()
    }

    #[test]
    fn each_agent_on_each_server_has_its_own_file() {
        let base = file("http://localhost:9881/", "local", "Owner(\"a b\")");
        assert!(base.starts_with("/cfg/ssh-history/Owner__a_b__-"), "{base}");
        assert!(base.ends_with(".txt"), "{base}");
        assert_eq!(
            base,
            file("http://localhost:9881/", "local", "Owner(\"a b\")")
        );
        assert_ne!(
            base,
            file(
                "https://release.api.golem.cloud/",
                "local",
                "Owner(\"a b\")"
            )
        );
        assert_ne!(
            base,
            file("http://localhost:9881/", "staging", "Owner(\"a b\")")
        );
        // Names that differ only in characters the file name cannot carry still differ.
        assert_ne!(
            base,
            file("http://localhost:9881/", "local", "Owner(\"a/b\")")
        );
        let long = file("s", "e", &"x".repeat(300));
        assert_eq!(Path::new(&long).file_name().unwrap().len(), 40 + 1 + 16 + 4);
    }

    #[test]
    fn spaced_and_local_lines_are_not_recorded() {
        assert!(is_recorded("echo hi"));
        assert!(is_recorded("cat <<EOF\nexit\nEOF"));
        assert!(is_recorded("help cd"));
        for line in [" echo secret", "exit", "exit 3", "help", "  tools  "] {
            assert!(!is_recorded(line), "{line:?}");
        }
    }

    fn recalled(history: &SessionHistory) -> Vec<String> {
        history
            .search(SearchQuery::everything(SearchDirection::Forward, None))
            .unwrap()
            .into_iter()
            .map(|item| item.command_line)
            .collect()
    }

    #[test]
    fn commands_come_back_in_a_later_session() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ssh-history").join("agent.txt");
        let mut first = SessionHistory::open(&path).unwrap();
        for line in [
            "echo one",
            " echo hidden",
            "help",
            "cat <<EOF\ntwo\nEOF",
            "exit",
        ] {
            first.save(HistoryItem::from_command_line(line)).unwrap();
        }
        first.sync().unwrap();
        drop(first);

        let second = SessionHistory::open(&path).unwrap();
        assert_eq!(recalled(&second), vec!["echo one", "cat <<EOF\ntwo\nEOF"]);
    }

    #[test]
    fn two_sessions_on_one_agent_both_keep_their_commands() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ssh-history").join("agent.txt");
        let mut one = SessionHistory::open(&path).unwrap();
        let mut two = SessionHistory::open(&path).unwrap();
        one.save(HistoryItem::from_command_line("echo from-one"))
            .unwrap();
        two.save(HistoryItem::from_command_line("echo from-two"))
            .unwrap();
        one.sync().unwrap();
        two.sync().unwrap();
        drop((one, two));

        let mut later = recalled(&SessionHistory::open(&path).unwrap());
        later.sort();
        assert_eq!(later, vec!["echo from-one", "echo from-two"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_history_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ssh-history").join("agent.txt");
        let mut history = SessionHistory::open(&path).unwrap();
        history
            .save(HistoryItem::from_command_line("echo one"))
            .unwrap();
        history.sync().unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_history_file_that_others_could_read_is_closed_to_them() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("ssh-history");
        let path = folder.join("agent.txt");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&path, "echo earlier\n").unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let history = SessionHistory::open(&path).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&folder), 0o700);
        assert_eq!(recalled(&history), vec!["echo earlier"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_history_file_is_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("somewhere-else.txt");
        std::fs::write(&target, "untouched\n").unwrap();
        let folder = directory.path().join("ssh-history");
        let path = folder.join("agent.txt");
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let error = SessionHistory::open(&path)
            .err()
            .expect("a link is refused");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
    }

    #[test]
    fn a_path_that_cannot_be_created_is_an_error_and_memory_still_works() {
        let directory = tempfile::tempdir().unwrap();
        let blocker = directory.path().join("file");
        std::fs::write(&blocker, "").unwrap();
        assert!(SessionHistory::open(&blocker.join("ssh-history").join("agent.txt")).is_err());
        let mut memory = SessionHistory::in_memory();
        memory
            .save(HistoryItem::from_command_line("echo one"))
            .unwrap();
        assert_eq!(recalled(&memory), vec!["echo one"]);
    }
}

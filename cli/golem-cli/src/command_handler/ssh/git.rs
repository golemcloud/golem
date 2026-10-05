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

//! The git branch of the session's directory, read from the agent's files with a short helper
//! script.

use super::completion::Fetch;

/// Prints the `HEAD` file of the repository the current directory is in, and fails outside one.
/// It uses only what the shell has built in. A repository here is a directory with a regular
/// `.git` directory, the only kind the built-in git tool works with.
pub const HEAD_SCRIPT: &str = "d=$PWD; \
     while [ ! -f \"$d/.git/HEAD\" ] && [ \"$d\" != / ]; do d=${d%/*}; d=${d:-/}; done; \
     [ -f \"$d/.git/HEAD\" ] && read -r head < \"$d/.git/HEAD\" && printf '%s\\n' \"$head\"";

/// The branch a `HEAD` file names, or the start of the commit id when no branch is checked out.
pub fn branch_of(head: &str) -> Option<String> {
    let head = head.trim();
    // The file is the agent's; nothing a terminal would act on may reach the prompt.
    if head.chars().any(char::is_control) {
        return None;
    }
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return (!branch.is_empty()).then(|| branch.to_string());
    }
    let commit = head.len() >= 7 && head.chars().all(|digit| digit.is_ascii_hexdigit());
    commit.then(|| head[..7].to_string())
}

/// Asks the agent for the branch of `cwd`. `None` outside a repository and when the agent does
/// not answer.
pub fn branch(fetch: &dyn Fetch, cwd: &str) -> Option<String> {
    branch_of(&fetch.run(cwd, HEAD_SCRIPT)?)
}

#[cfg(test)]
mod tests {
    use super::{HEAD_SCRIPT, branch, branch_of};
    use crate::command_handler::ssh::completion::Fetch;
    use std::sync::Mutex;
    use test_r::test;

    #[test]
    fn a_head_file_names_a_branch_or_a_commit() {
        for (head, expected) in [
            ("ref: refs/heads/main\n", Some("main")),
            ("ref: refs/heads/feature/prompt", Some("feature/prompt")),
            // No branch is checked out: the start of the commit id.
            (
                "9f2c1e7ab04d5c6e8f1a2b3c4d5e6f708192a3b4\n",
                Some("9f2c1e7"),
            ),
            ("ref: refs/heads/", None),
            ("ref: refs/tags/v1", None),
            ("not a head file", None),
            ("", None),
            // Nothing a terminal would act on reaches the prompt.
            ("ref: refs/heads/a\x1b[31mb", None),
        ] {
            assert_eq!(branch_of(head).as_deref(), expected, "{head:?}");
        }
    }

    struct Agent {
        answer: Option<&'static str>,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl Fetch for Agent {
        fn run(&self, cwd: &str, script: &str) -> Option<String> {
            self.calls
                .lock()
                .unwrap()
                .push((cwd.to_string(), script.to_string()));
            self.answer.map(str::to_string)
        }
    }

    #[test]
    fn the_branch_is_read_with_one_script_from_the_sessions_directory() {
        let agent = Agent {
            answer: Some("ref: refs/heads/dev\n"),
            calls: Mutex::default(),
        };
        assert_eq!(branch(&agent, "/work/repo").as_deref(), Some("dev"));
        assert_eq!(
            *agent.calls.lock().unwrap(),
            vec![("/work/repo".to_string(), HEAD_SCRIPT.to_string())]
        );

        // Outside a repository the script fails, and so does an agent that does not answer.
        let silent = Agent {
            answer: None,
            calls: Mutex::default(),
        };
        assert_eq!(branch(&silent, "/work"), None);
    }
}

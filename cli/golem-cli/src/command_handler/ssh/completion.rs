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

//! Tab completion for the prompt: command names and remote paths, fetched from the agent with
//! short helper scripts and remembered for as long as they can be trusted.

use super::syntax::{Position, cursor_word, is_reserved_word};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Lists every command name the agent's shell knows, one per line.
pub const COMMANDS_SCRIPT: &str = "compgen -c";

/// Runs helper scripts on the agent.
pub trait Fetch: Send + Sync {
    /// The script's stdout when it ran from `cwd` and exited 0 in time; `None` otherwise.
    fn run(&self, cwd: &str, script: &str) -> Option<String>;
}

/// What the editor knows about the agent between commands.
#[derive(Debug, Default)]
struct View {
    cwd: String,
    commands: Option<Arc<BTreeSet<String>>>,
    /// The names of the tools bound to the agent, once they are known.
    tools: Arc<BTreeSet<String>>,
    /// Directory listings by the directory part as typed: `""`, `"sub/"`, `"/tmp/"`.
    listings: HashMap<String, Arc<Vec<String>>>,
}

/// What a completion is. The block look's list shows it beside the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    /// A tool bound to the agent.
    Tool,
    Command,
    Directory,
    File,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Keyword => "keyword",
            Kind::Tool => "tool",
            Kind::Command => "command",
            Kind::Directory => "dir",
            Kind::File => "file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The text that replaces `start..end` of the line.
    pub value: String,
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// False after a directory, so the next Tab continues inside it.
    pub append_space: bool,
}

#[derive(Clone)]
pub struct Completions {
    view: Arc<Mutex<View>>,
    fetch: Arc<dyn Fetch>,
}

impl Completions {
    pub fn new(fetch: Arc<dyn Fetch>, cwd: &str) -> Self {
        let view = View {
            cwd: cwd.to_string(),
            ..View::default()
        };
        Self {
            view: Arc::new(Mutex::new(view)),
            fetch,
        }
    }

    fn view(&self) -> MutexGuard<'_, View> {
        self.view.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A command ran and ended in `cwd`: it may have changed any file, so the listings go.
    pub fn command_finished(&self, cwd: &str) {
        let mut view = self.view();
        view.cwd = cwd.to_string();
        view.listings.clear();
    }

    /// Names the tools bound to the agent, which complete as commands.
    pub fn set_tools(&self, names: impl IntoIterator<Item = String>) {
        self.view().tools = Arc::new(names.into_iter().collect());
    }

    /// The agent's command names, when they have been fetched.
    pub fn commands(&self) -> Option<Arc<BTreeSet<String>>> {
        self.view().commands.clone()
    }

    /// Fetches the agent's command names once.
    pub fn load_commands(&self) -> Option<Arc<BTreeSet<String>>> {
        if let Some(commands) = self.commands() {
            return Some(commands);
        }
        let cwd = self.view().cwd.clone();
        let output = self.fetch.run(&cwd, COMMANDS_SCRIPT)?;
        let commands: BTreeSet<String> = lines(&output).map(str::to_string).collect();
        if commands.is_empty() {
            return None;
        }
        let commands = Arc::new(commands);
        self.view().commands = Some(commands.clone());
        Some(commands)
    }

    fn listing(&self, directory: &str) -> Option<Arc<Vec<String>>> {
        if let Some(listing) = self.view().listings.get(directory) {
            return Some(listing.clone());
        }
        let cwd = self.view().cwd.clone();
        let output = self.fetch.run(&cwd, &listing_script(directory))?;
        let listing = Arc::new(lines(&output).map(str::to_string).collect::<Vec<_>>());
        self.view()
            .listings
            .insert(directory.to_string(), listing.clone());
        Some(listing)
    }

    /// The completions of the word that ends at `cursor`. Empty when there is nothing to
    /// offer or the agent did not answer.
    pub fn complete(&self, line: &str, cursor: usize) -> Vec<Candidate> {
        let Some(word) = cursor_word(line, cursor) else {
            return Vec::new();
        };
        let candidate = |value: String, kind: Kind, append_space: bool| Candidate {
            value,
            kind,
            start: word.start,
            end: cursor,
            append_space,
        };
        if word.position == Position::Command && !word.text.contains('/') {
            let Some(commands) = self.load_commands() else {
                return Vec::new();
            };
            let tools = self.view().tools.clone();
            return commands
                .iter()
                .filter(|name| name.starts_with(&word.text))
                .map(|name| {
                    let kind = if is_reserved_word(name) {
                        Kind::Keyword
                    } else if tools.contains(name) {
                        Kind::Tool
                    } else {
                        Kind::Command
                    };
                    candidate(name.clone(), kind, true)
                })
                .collect();
        }
        let directory = word
            .text
            .rfind('/')
            .map_or("", |slash| &word.text[..=slash]);
        let hidden = word.text[directory.len()..].starts_with('.');
        let Some(listing) = self.listing(directory) else {
            return Vec::new();
        };
        listing
            .iter()
            .filter(|entry| entry.starts_with(&word.text))
            .filter(|entry| hidden || !entry[directory.len()..].starts_with('.'))
            .map(|entry| {
                let directory = entry.ends_with('/');
                let kind = if directory {
                    Kind::Directory
                } else {
                    Kind::File
                };
                candidate(escape_word(entry), kind, !directory)
            })
            .collect()
    }
}

fn lines(output: &str) -> impl Iterator<Item = &str> {
    output.lines().filter(|line| !line.is_empty())
}

/// Lists the entries of `directory` (as typed, ending in `/`, or empty for the current
/// directory), hidden ones included, one per line and with a trailing `/` on directories.
pub fn listing_script(directory: &str) -> String {
    let directory = format!("'{}'", directory.replace('\'', "'\\''"));
    format!(
        "for p in {directory}* {directory}.[!.]*; do \
         if [ -d \"$p\" ]; then printf '%s/\\n' \"$p\"; \
         elif [ -e \"$p\" ] || [ -L \"$p\" ]; then printf '%s\\n' \"$p\"; fi; done"
    )
}

/// Escapes `text` so that typing it at the prompt names exactly that path.
pub fn escape_word(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii()
            && !character.is_ascii_alphanumeric()
            && !"_./-+:@%,=".contains(character)
        {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS_SCRIPT, Completions, Fetch, Kind, escape_word, listing_script};
    use std::sync::{Arc, Mutex};
    use test_r::test;

    /// Answers scripts from a table and records every call.
    #[derive(Default)]
    struct Agent {
        answers: Vec<(String, Option<String>)>,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl Agent {
        fn answering(answers: &[(&str, Option<&str>)]) -> Arc<Self> {
            Arc::new(Self {
                answers: answers
                    .iter()
                    .map(|(script, output)| (script.to_string(), output.map(str::to_string)))
                    .collect(),
                calls: Mutex::default(),
            })
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetch for Agent {
        fn run(&self, cwd: &str, script: &str) -> Option<String> {
            self.calls
                .lock()
                .unwrap()
                .push((cwd.to_string(), script.to_string()));
            self.answers
                .iter()
                .find(|(known, _)| known == script)
                .and_then(|(_, output)| output.clone())
        }
    }

    fn values(completions: &Completions, line: &str) -> Vec<(String, bool)> {
        completions
            .complete(line, line.len())
            .into_iter()
            .map(|candidate| (candidate.value, candidate.append_space))
            .collect()
    }

    #[test]
    fn command_names_are_fetched_once_and_filtered_by_prefix() {
        let agent = Agent::answering(&[(COMMANDS_SCRIPT, Some("grep\ngrowl\nls\ngrep\n\n"))]);
        let completions = Completions::new(agent.clone(), "/work");
        assert_eq!(
            values(&completions, "gr"),
            vec![("grep".to_string(), true), ("growl".to_string(), true)]
        );
        assert_eq!(
            values(&completions, "ls | l"),
            vec![("ls".to_string(), true)]
        );
        assert_eq!(
            agent.calls(),
            vec![("/work".to_string(), COMMANDS_SCRIPT.to_string())]
        );
        let replaced = &completions.complete("ls | l", 6)[0];
        assert_eq!((replaced.start, replaced.end), (5, 6));
    }

    #[test]
    fn paths_complete_from_one_listing_per_directory() {
        let listing = listing_script("/tmp/");
        let agent = Agent::answering(&[(
            listing.as_str(),
            Some("/tmp/alpha.txt\n/tmp/album/\n/tmp/beta\n/tmp/.hidden\n/tmp/my file\n"),
        )]);
        let completions = Completions::new(agent.clone(), "");
        assert_eq!(
            values(&completions, "cat /tmp/al"),
            vec![
                ("/tmp/alpha.txt".to_string(), true),
                ("/tmp/album/".to_string(), false)
            ]
        );
        // Hidden entries appear only once a dot is typed; a space is escaped.
        assert_eq!(
            values(&completions, "cat /tmp/."),
            vec![("/tmp/.hidden".to_string(), true)]
        );
        assert_eq!(
            values(&completions, "cat /tmp/m"),
            vec![("/tmp/my\\ file".to_string(), true)]
        );
        assert_eq!(agent.calls().len(), 1);
    }

    #[test]
    fn every_completion_says_what_it_is() {
        let listing = listing_script("");
        let agent = Agent::answering(&[
            (COMMANDS_SCRIPT, Some("fi\nfile\nfind\nfixture\n")),
            (listing.as_str(), Some("notes.txt\nsrc/\n")),
        ]);
        let completions = Completions::new(agent, "");
        let kinds = |line: &str| -> Vec<Kind> {
            completions
                .complete(line, line.len())
                .into_iter()
                .map(|candidate| candidate.kind)
                .collect()
        };
        // Until the agent's tools are known, a tool reads as any other command.
        assert_eq!(
            kinds("fi"),
            vec![Kind::Keyword, Kind::Command, Kind::Command, Kind::Command]
        );
        completions.set_tools(["fixture".to_string()]);
        assert_eq!(
            kinds("fi"),
            vec![Kind::Keyword, Kind::Command, Kind::Command, Kind::Tool]
        );
        assert_eq!(kinds("cat "), vec![Kind::File, Kind::Directory]);
        assert_eq!(
            [
                Kind::Keyword,
                Kind::Tool,
                Kind::Command,
                Kind::Directory,
                Kind::File
            ]
            .map(Kind::label),
            ["keyword", "tool", "command", "dir", "file"]
        );
    }

    #[test]
    fn a_finished_command_drops_the_listings_and_moves_the_directory() {
        let listing = listing_script("");
        let agent = Agent::answering(&[(listing.as_str(), Some("a\n"))]);
        let completions = Completions::new(agent.clone(), "/one");
        values(&completions, "cat ");
        values(&completions, "cat ");
        completions.command_finished("/two");
        values(&completions, "cat ");
        let directories: Vec<String> = agent.calls().into_iter().map(|(cwd, _)| cwd).collect();
        assert_eq!(directories, vec!["/one".to_string(), "/two".to_string()]);
    }

    #[test]
    fn an_agent_that_does_not_answer_gives_no_completions_and_is_asked_again() {
        let agent = Agent::answering(&[(COMMANDS_SCRIPT, None)]);
        let completions = Completions::new(agent.clone(), "");
        assert_eq!(values(&completions, "gr"), vec![]);
        assert_eq!(values(&completions, "cat /tmp/a"), vec![]);
        assert_eq!(completions.commands(), None);
        assert_eq!(values(&completions, "gr"), vec![]);
        assert_eq!(agent.calls().len(), 3);
    }

    #[test]
    fn nothing_is_fetched_where_no_word_can_be_completed() {
        let agent = Agent::answering(&[]);
        let completions = Completions::new(agent.clone(), "");
        assert_eq!(values(&completions, "echo 'ab"), vec![]);
        assert_eq!(values(&completions, "echo $HOME/a"), vec![]);
        assert!(agent.calls().is_empty());
    }

    #[test]
    fn a_path_in_command_position_completes_as_a_path() {
        let listing = listing_script("./");
        let agent = Agent::answering(&[(listing.as_str(), Some("./run.sh\n"))]);
        let completions = Completions::new(agent, "");
        assert_eq!(
            values(&completions, "./r"),
            vec![("./run.sh".to_string(), true)]
        );
    }

    #[test]
    fn the_listing_script_quotes_the_directory() {
        assert_eq!(
            listing_script("it's/"),
            "for p in 'it'\\''s/'* 'it'\\''s/'.[!.]*; do \
             if [ -d \"$p\" ]; then printf '%s/\\n' \"$p\"; \
             elif [ -e \"$p\" ] || [ -L \"$p\" ]; then printf '%s\\n' \"$p\"; fi; done"
        );
    }

    #[test]
    fn inserted_paths_are_escaped_for_the_shell() {
        assert_eq!(escape_word("plain-1.2_x/y"), "plain-1.2_x/y");
        assert_eq!(escape_word("a b$c'd\"e"), "a\\ b\\$c\\'d\\\"e");
        assert_eq!(escape_word("caf\u{e9}"), "caf\u{e9}");
    }
}

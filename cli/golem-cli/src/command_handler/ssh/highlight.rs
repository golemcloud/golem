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

//! Colours for the line being typed, from the lexer's tokens. It never calls the agent.

use super::syntax::{Token, TokenKind, scan};
use std::collections::BTreeSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Plain,
    /// A command name the agent knows.
    Command,
    /// A command name the agent does not list.
    UnknownCommand,
    Reserved,
    Quoted,
    Expansion,
    Operator,
    Redirect,
    Comment,
    Heredoc,
}

/// Commands the session handles itself, which the agent does not list.
const LOCAL_COMMANDS: [&str; 1] = ["tools"];

/// The paint of every byte of `line`, in order and without gaps. `commands` is the agent's
/// command names once they are known; until then no command name is judged.
pub fn paints(line: &str, commands: Option<&BTreeSet<String>>) -> Vec<(Paint, Range<usize>)> {
    let tokens = scan(line).tokens;
    let text = |token: &Token| &line[token.start..token.end];
    let command = TokenKind::Word { command: true };
    // Functions the input defines are commands too: `name()` and `function name`.
    let mut defined: Vec<&str> = tokens
        .windows(3)
        .filter(|w| w[0].kind == command && text(&w[1]) == "(" && text(&w[2]) == ")")
        .map(|w| text(&w[0]))
        .collect();
    defined.extend(
        tokens
            .windows(2)
            .filter(|w| w[0].kind == TokenKind::Reserved && text(&w[0]) == "function")
            .map(|w| text(&w[1])),
    );

    let mut paints = Vec::with_capacity(tokens.len() * 2 + 1);
    let mut end = 0;
    for token in &tokens {
        if token.start > end {
            paints.push((Paint::Plain, end..token.start));
        }
        let word = text(token);
        let paint = match token.kind {
            TokenKind::Word { command: true } => match commands {
                Some(known) if !word.contains('/') => {
                    if known.contains(word)
                        || LOCAL_COMMANDS.contains(&word)
                        || defined.contains(&word)
                    {
                        Paint::Command
                    } else {
                        Paint::UnknownCommand
                    }
                }
                _ => Paint::Plain,
            },
            TokenKind::Word { command: false } => Paint::Plain,
            TokenKind::Reserved => Paint::Reserved,
            TokenKind::Quoted => Paint::Quoted,
            TokenKind::Expansion => Paint::Expansion,
            TokenKind::Operator => Paint::Operator,
            TokenKind::Redirect => Paint::Redirect,
            TokenKind::Comment => Paint::Comment,
            TokenKind::Heredoc => Paint::Heredoc,
        };
        paints.push((paint, token.start..token.end));
        end = token.end;
    }
    if end < line.len() {
        paints.push((Paint::Plain, end..line.len()));
    }
    paints
}

#[cfg(test)]
mod tests {
    use super::{Paint, paints};
    use std::collections::BTreeSet;
    use test_r::test;

    fn painted<'a>(line: &'a str, commands: Option<&[&str]>) -> Vec<(Paint, &'a str)> {
        let commands: Option<BTreeSet<String>> =
            commands.map(|names| names.iter().map(|name| name.to_string()).collect());
        paints(line, commands.as_ref())
            .into_iter()
            .map(|(paint, range)| (paint, &line[range]))
            .collect()
    }

    #[test]
    fn every_byte_is_painted_once_and_in_order() {
        let line = "  ls -l \"$HOME\" | nope > out # \u{2713} ";
        let mut end = 0;
        for (_, range) in paints(line, None) {
            assert_eq!(range.start, end);
            assert!(range.end > range.start);
            end = range.end;
        }
        assert_eq!(end, line.len());
        assert!(paints("", None).is_empty());
    }

    #[test]
    fn command_names_are_judged_only_once_the_list_is_known() {
        assert_eq!(painted("nope", None), vec![(Paint::Plain, "nope")]);
        assert_eq!(
            painted("ls | nope", Some(&["ls"])),
            vec![
                (Paint::Command, "ls"),
                (Paint::Plain, " "),
                (Paint::Operator, "|"),
                (Paint::Plain, " "),
                (Paint::UnknownCommand, "nope"),
            ]
        );
    }

    #[test]
    fn paths_local_commands_and_defined_functions_are_not_unknown() {
        let known = Some(&["ls"][..]);
        assert_eq!(painted("./run.sh", known), vec![(Paint::Plain, "./run.sh")]);
        assert_eq!(painted("tools", known), vec![(Paint::Command, "tools")]);
        let defined = painted("f() { ls; }; f", known);
        assert_eq!(defined.first(), Some(&(Paint::Command, "f")));
        assert_eq!(defined.last(), Some(&(Paint::Command, "f")));
        let named = painted("function g { ls; }; g", known);
        assert_eq!(named.last(), Some(&(Paint::Command, "g")));
    }

    #[test]
    fn the_parts_of_a_command_get_their_own_paint() {
        assert_eq!(
            painted("if x; then echo 'a' $b >f; fi # c", Some(&["echo", "x"])),
            vec![
                (Paint::Reserved, "if"),
                (Paint::Plain, " "),
                (Paint::Command, "x"),
                (Paint::Operator, ";"),
                (Paint::Plain, " "),
                (Paint::Reserved, "then"),
                (Paint::Plain, " "),
                (Paint::Command, "echo"),
                (Paint::Plain, " "),
                (Paint::Quoted, "'a'"),
                (Paint::Plain, " "),
                (Paint::Expansion, "$b"),
                (Paint::Plain, " "),
                (Paint::Redirect, ">"),
                (Paint::Plain, "f"),
                (Paint::Operator, ";"),
                (Paint::Plain, " "),
                (Paint::Reserved, "fi"),
                (Paint::Plain, " "),
                (Paint::Comment, "# c"),
            ]
        );
    }
}

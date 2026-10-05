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

//! The line editor behind the prompt: `reedline` wired to the session's lexer, completions,
//! colours and history.

use super::completion::Completions;
use super::highlight::{Paint, paints};
use super::history::SessionHistory;
use super::syntax::is_complete;
use reedline::{
    Color, ColumnarMenu, Completer, CompletionResult, DefaultHinter, Emacs, Highlighter, KeyCode,
    KeyModifiers, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Span, StyledText, Suggestion,
    ValidationResult, Validator, default_emacs_keybindings,
};
use std::borrow::Cow;

const COMPLETION_MENU: &str = "completion_menu";

/// Builds the editor. It paints on stderr. Enter continues unfinished input on a new line,
/// Tab completes from the agent, and typed text is coloured when `colorize` is set.
pub fn build(colorize: bool, history: SessionHistory, completions: Completions) -> Reedline {
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    let editor = Reedline::create()
        .with_history(Box::new(history))
        .with_hinter(Box::new(
            DefaultHinter::default().with_style(Color::DarkGray.normal()),
        ))
        .with_validator(Box::new(Finished))
        .with_completer(Box::new(FromAgent(completions.clone())))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default().with_name(COMPLETION_MENU),
        )))
        .with_quick_completions(true)
        .with_partial_completions(true)
        .with_edit_mode(Box::new(Emacs::new(keybindings)))
        .with_ansi_colors(colorize);
    if colorize {
        editor.with_highlighter(Box::new(Coloured(completions)))
    } else {
        editor.with_highlighter(Box::new(Uncoloured))
    }
}

/// Enter submits finished input and continues anything else on a new line.
struct Finished;

impl Validator for Finished {
    fn validate(&self, line: &str) -> ValidationResult {
        if is_complete(line) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}

struct FromAgent(Completions);

impl Completer for FromAgent {
    fn complete(&mut self, line: &str, position: usize) -> CompletionResult {
        let suggestions: Vec<Suggestion> = self
            .0
            .complete(line, position)
            .into_iter()
            .map(|candidate| Suggestion {
                value: candidate.value,
                span: Span::new(candidate.start, candidate.end),
                append_whitespace: candidate.append_space,
                ..Suggestion::default()
            })
            .collect();
        CompletionResult::fresh(suggestions)
    }
}

struct Coloured(Completions);

impl Highlighter for Coloured {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let commands = self.0.commands();
        let mut styled = StyledText::new();
        for (paint, range) in paints(line, commands.as_deref()) {
            let style = match paint {
                Paint::Plain => Default::default(),
                Paint::Command => Color::Green.normal(),
                Paint::UnknownCommand => Color::Red.normal(),
                Paint::Reserved => Color::Purple.bold(),
                Paint::Operator => Color::Purple.normal(),
                Paint::Redirect => Color::Blue.normal(),
                Paint::Quoted | Paint::Heredoc => Color::Yellow.normal(),
                Paint::Expansion => Color::Cyan.normal(),
                Paint::Comment => Color::DarkGray.normal(),
            };
            styled.push((style, line[range].to_string()));
        }
        styled
    }
}

struct Uncoloured;

impl Highlighter for Uncoloured {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        styled.push((Default::default(), line.to_string()));
        styled
    }
}

/// The prompt text before the first line, and a dim marker before each continued line.
pub struct SshPrompt(pub String);

impl Prompt for SshPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.0)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("\u{b7} ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let failing = match history_search.status {
            PromptHistorySearchStatus::Failing => "failing ",
            PromptHistorySearchStatus::Passing => "",
        };
        Cow::Owned(format!(
            "({failing}reverse-search: {}) ",
            history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Coloured, Finished, FromAgent};
    use crate::command_handler::ssh::completion::{COMMANDS_SCRIPT, Completions, Fetch};
    use reedline::{Completer, Highlighter, ValidationResult, Validator};
    use std::sync::Arc;
    use test_r::test;

    struct Agent;

    impl Fetch for Agent {
        fn run(&self, _cwd: &str, script: &str) -> Option<String> {
            (script == COMMANDS_SCRIPT).then(|| "grep\nls\n".to_string())
        }
    }

    #[test]
    fn enter_continues_only_unfinished_input() {
        assert!(matches!(
            Finished.validate("echo hi"),
            ValidationResult::Complete
        ));
        assert!(matches!(
            Finished.validate("cat <<EOF\nhi"),
            ValidationResult::Incomplete
        ));
    }

    #[test]
    fn tab_replaces_the_word_under_the_cursor() {
        let mut completer = FromAgent(Completions::new(Arc::new(Agent), ""));
        let result = completer.complete("ls | gr", 7);
        let suggestions = result.suggestions();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "grep");
        assert_eq!((suggestions[0].span.start, suggestions[0].span.end), (5, 7));
        assert!(suggestions[0].append_whitespace);
    }

    #[test]
    fn colouring_keeps_the_text_unchanged() {
        let completions = Completions::new(Arc::new(Agent), "");
        completions.load_commands();
        let line = "ls 'a b' | nope # \u{2713}";
        let styled = Coloured(completions).highlight(line, 0);
        assert_eq!(styled.raw_string(), line);
        assert!(styled.buffer.len() > 4);
    }
}

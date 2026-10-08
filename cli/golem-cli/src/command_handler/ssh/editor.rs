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
use super::look::{self, Palette};
use super::syntax::is_complete;
use crossterm::event::{Event, KeyEvent};
use reedline::{
    Color, ColumnarMenu, Completer, CompletionResult, DefaultHinter, EditCommand, EditMode, Emacs,
    Highlighter, IdeMenu, KeyCode, KeyModifiers, MenuBuilder, Prompt, PromptEditMode,
    PromptHistorySearch, PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu,
    ReedlineRawEvent, Span, StyledText, Suggestion, ValidationResult, Validator,
    default_emacs_keybindings,
};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

const COMPLETION_MENU: &str = "completion_menu";

/// Builds the editor. It paints on stderr. Enter continues unfinished input on a new line and
/// Tab completes from the agent. With a palette the session has the slab look: typed text is
/// coloured and the completions are a bordered list, purple where it is selected, that says
/// what each one is.
pub fn build(
    palette: Option<Palette>,
    history: SessionHistory,
    completions: Completions,
) -> Reedline {
    let colorize = palette.is_some();
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
        .with_completer(Box::new(FromAgent {
            completions: completions.clone(),
            kinds: colorize,
        }))
        .with_menu(completion_menu(palette))
        .with_quick_completions(true)
        .with_partial_completions(true)
        .with_edit_mode(Box::new(WithoutLateAnswer::of(Emacs::new(keybindings))))
        .with_ansi_colors(colorize);
    if colorize {
        editor.with_highlighter(Box::new(Coloured(completions)))
    } else {
        editor.with_highlighter(Box::new(Uncoloured))
    }
}

/// The edit mode `keys`, without a terminal's late answer about its background.
///
/// A terminal that answers after the session has stopped waiting sends its answer to the line
/// editor. The terminal layer under the editor reads the `ESC ]` that opens the answer as
/// Alt+`]`, each character after it as a typed one, and the BEL or `ESC \` that ends it as
/// Ctrl+G or Alt+`\`. So what comes after Alt+`]` is held back for as long as it reads like
/// that answer. When the answer ends, what was held is dropped. When anything else comes, it
/// was typed, and it is given back as text: nothing typed is lost or changed.
struct WithoutLateAnswer<M> {
    keys: M,
    /// What came since Alt+`]`, while it can still be the answer.
    held: Option<String>,
}

impl<M: EditMode> WithoutLateAnswer<M> {
    fn of(keys: M) -> Self {
        Self { keys, held: None }
    }

    fn pass(&mut self, event: Event) -> ReedlineEvent {
        ReedlineRawEvent::try_from(event)
            .map_or(ReedlineEvent::None, |event| self.keys.parse_event(event))
    }
}

impl<M: EditMode> EditMode for WithoutLateAnswer<M> {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let event = Event::from(event);
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        else {
            // A window that changes size says nothing about what the keys around it are.
            return self.pass(event);
        };
        let opens = (code, modifiers) == (KeyCode::Char(']'), KeyModifiers::ALT);
        let Some(mut held) = self.held.take() else {
            if opens {
                self.held = Some(String::new());
                return ReedlineEvent::None;
            }
            return self.pass(event);
        };
        match (code, modifiers) {
            (KeyCode::Char(character), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                held.push(character);
                if look::begins_background_answer(&held) {
                    self.held = Some(held);
                    return ReedlineEvent::None;
                }
                held.pop();
            }
            (KeyCode::Char('g'), KeyModifiers::CONTROL)
            | (KeyCode::Char('\\'), KeyModifiers::ALT)
                if look::parse_background(format!("\x1b]{held}").as_bytes()).is_some() =>
            {
                return ReedlineEvent::None;
            }
            _ => {}
        }
        // It was typed.
        let next = if opens {
            self.held = Some(String::new());
            ReedlineEvent::None
        } else {
            self.pass(event)
        };
        if held.is_empty() {
            next
        } else {
            ReedlineEvent::Multiple(vec![
                ReedlineEvent::Edit(vec![EditCommand::InsertString(held)]),
                next,
            ])
        }
    }

    fn edit_mode(&self) -> PromptEditMode {
        self.keys.edit_mode()
    }
}

/// The list Tab opens. Neither has a marker: the prompt stays as it is while the list is open.
/// The text and the background of the row the list has selected.
fn selected_colours(palette: Palette) -> (Color, Color) {
    match palette {
        Palette::Rich => (Color::Fixed(16), Color::Fixed(141)),
        Palette::Basic => (Color::Black, Color::LightPurple),
    }
}

fn completion_menu(palette: Option<Palette>) -> ReedlineMenu {
    if let Some(palette) = palette {
        let (text, background) = selected_colours(palette);
        let selected = text.on(background);
        ReedlineMenu::EngineCompleter(Box::new(
            IdeMenu::default()
                .with_name(COMPLETION_MENU)
                .with_marker("")
                .with_default_border()
                .with_padding(1)
                .with_text_style(Color::Default.normal())
                // What has been typed so far stands out in the same purple.
                .with_match_text_style(background.bold())
                .with_selected_text_style(selected)
                .with_selected_match_text_style(selected.bold()),
        ))
    } else {
        ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default()
                .with_name(COMPLETION_MENU)
                .with_marker(""),
        ))
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

struct FromAgent {
    completions: Completions,
    /// Whether the list says what each completion is.
    kinds: bool,
}

impl Completer for FromAgent {
    fn complete(&mut self, line: &str, position: usize) -> CompletionResult {
        let candidates = self.completions.complete(line, position);
        let width = candidates
            .iter()
            .map(|candidate| candidate.value.width())
            .max()
            .unwrap_or(0);
        let mut suggestions: Vec<Suggestion> = candidates
            .into_iter()
            .map(|candidate| Suggestion {
                // The names in one column, what each one is in the next.
                display_override: self.kinds.then(|| {
                    format!(
                        "{}{}  {}",
                        candidate.value,
                        " ".repeat(width - candidate.value.width()),
                        candidate.kind.label()
                    )
                }),
                value: candidate.value,
                span: Span::new(candidate.start, candidate.end),
                append_whitespace: candidate.append_space,
                ..Suggestion::default()
            })
            .collect();
        if suggestions.is_empty() {
            // With nothing to offer the menu would announce "NO RECORDS FOUND". One suggestion
            // that inserts nothing makes Tab do nothing instead.
            suggestions.push(Suggestion {
                span: Span::new(position, position),
                ..Suggestion::default()
            });
        }
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

/// What a prompt without colours shows where a command goes on to another line.
pub const PLAIN_CONTINUATION: &str = "\u{b7} ";

/// What the editor draws around the text being typed.
pub struct SshPrompt {
    /// Everything before the typed text, which starts after its last line.
    pub left: String,
    /// Kept at the right edge of the first line for as long as the typed text leaves it room.
    pub right: String,
    /// Starts each continued line.
    pub continuation: &'static str,
}

impl SshPrompt {
    /// One line of text and a dot before each continued line: the prompt without the block
    /// look.
    pub fn plain(text: String) -> Self {
        Self {
            left: text,
            right: String::new(),
            continuation: PLAIN_CONTINUATION,
        }
    }
}

impl Prompt for SshPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.right)
    }

    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.continuation)
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
    use super::{Coloured, Finished, FromAgent, SshPrompt, WithoutLateAnswer, selected_colours};
    use crate::command_handler::ssh::completion::{COMMANDS_SCRIPT, Completions, Fetch};
    use crate::command_handler::ssh::look::Palette;
    use crossterm::event::{Event, KeyEvent};
    use reedline::Color;
    use reedline::{
        Completer, EditCommand, EditMode, Emacs, Highlighter, KeyCode, KeyModifiers, Prompt,
        ReedlineEvent, ReedlineRawEvent, Suggestion, ValidationResult, Validator,
        default_emacs_keybindings,
    };
    use std::sync::Arc;
    use test_r::test;

    struct Agent;

    impl Fetch for Agent {
        fn run(&self, _cwd: &str, script: &str) -> Option<String> {
            (script == COMMANDS_SCRIPT).then(|| "for\ngrep\nls\nlsof\n".to_string())
        }
    }

    fn completer(kinds: bool) -> FromAgent {
        FromAgent {
            completions: Completions::new(Arc::new(Agent), ""),
            kinds,
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
        let mut completer = completer(false);
        let result = completer.complete("ls | gr", 7);
        let suggestions = result.suggestions();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "grep");
        assert_eq!((suggestions[0].span.start, suggestions[0].span.end), (5, 7));
        assert!(suggestions[0].append_whitespace);
    }

    #[test]
    fn tab_with_nothing_to_offer_changes_nothing() {
        let mut completer = completer(true);
        for (line, position) in [("zzz", 3), ("echo 'ab", 8)] {
            let result = completer.complete(line, position);
            let suggestions = result.suggestions();
            assert_eq!(suggestions.len(), 1, "{line:?}");
            assert_eq!(suggestions[0].value, "");
            assert_eq!(suggestions[0].display_value(), "");
            assert_eq!(
                (suggestions[0].span.start, suggestions[0].span.end),
                (position, position)
            );
            assert!(!suggestions[0].append_whitespace);
        }
    }

    #[test]
    fn the_block_list_names_the_kind_beside_each_completion() {
        let mut blocks = completer(true);
        let result = blocks.complete("l", 1);
        let shown: Vec<&str> = result
            .suggestions()
            .iter()
            .map(Suggestion::display_value)
            .collect();
        assert_eq!(shown, ["ls    command", "lsof  command"]);
        // What Tab inserts is the name alone.
        assert_eq!(result.suggestions()[0].value, "ls");
        let result = blocks.complete("fo", 2);
        assert_eq!(result.suggestions()[0].display_value(), "for  keyword");

        let mut plain = completer(false);
        let result = plain.complete("l", 1);
        assert_eq!(result.suggestions()[0].display_override, None);
    }

    #[test]
    fn the_selected_row_of_the_list_is_purple() {
        assert_eq!(
            selected_colours(Palette::Rich),
            (Color::Fixed(16), Color::Fixed(141))
        );
        assert_eq!(
            selected_colours(Palette::Basic),
            (Color::Black, Color::LightPurple)
        );
    }

    #[test]
    fn the_prompt_draws_what_it_is_given() {
        let plain = SshPrompt::plain("agent \u{276f} ".to_string());
        assert_eq!(plain.render_prompt_left(), "agent \u{276f} ");
        assert_eq!(plain.render_prompt_right(), "");
        assert_eq!(plain.render_prompt_multiline_indicator(), "\u{b7} ");

        let blocks = SshPrompt {
            left: "blocks\n> ".to_string(),
            right: "result".to_string(),
            continuation: "| ",
        };
        assert_eq!(blocks.render_prompt_left(), "blocks\n> ");
        assert_eq!(blocks.render_prompt_right(), "result");
        assert_eq!(blocks.render_prompt_multiline_indicator(), "| ");
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

    fn editing() -> WithoutLateAnswer<Emacs> {
        WithoutLateAnswer::of(Emacs::new(default_emacs_keybindings()))
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> ReedlineRawEvent {
        ReedlineRawEvent::try_from(Event::Key(KeyEvent::new(code, modifiers))).unwrap()
    }

    /// A character as the terminal layer reports it typed: a capital comes with Shift.
    fn typed(character: char) -> ReedlineRawEvent {
        let modifiers = if character.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        key(KeyCode::Char(character), modifiers)
    }

    /// `ESC ]`, which opens a terminal's answer, as the terminal layer reads it.
    fn alt_bracket() -> ReedlineRawEvent {
        key(KeyCode::Char(']'), KeyModifiers::ALT)
    }

    /// BEL and `ESC \`, either of which ends the answer, as the terminal layer reads them.
    fn ends() -> [(KeyCode, KeyModifiers); 2] {
        [
            (KeyCode::Char('g'), KeyModifiers::CONTROL),
            (KeyCode::Char('\\'), KeyModifiers::ALT),
        ]
    }

    fn inserted(character: char) -> ReedlineEvent {
        ReedlineEvent::Edit(vec![EditCommand::InsertChar(character)])
    }

    fn given_back(text: &str, then: ReedlineEvent) -> ReedlineEvent {
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertString(text.to_string())]),
            then,
        ])
    }

    #[test]
    fn a_late_answer_about_the_background_does_not_reach_the_line() {
        for answer in [
            "11;rgb:1414/1313/1b1b",
            "11;rgba:1e/1e/1e/ff",
            "11;rgb:FFFF/0/8",
        ] {
            for (code, modifiers) in ends() {
                let mut mode = editing();
                // What is typed before the answer and after it is typed as always.
                assert_eq!(mode.parse_event(typed('l')), inserted('l'));
                assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
                for character in answer.chars() {
                    assert_eq!(
                        mode.parse_event(typed(character)),
                        ReedlineEvent::None,
                        "{character:?} of {answer:?}"
                    );
                }
                assert_eq!(mode.parse_event(key(code, modifiers)), ReedlineEvent::None);
                assert_eq!(mode.parse_event(typed('s')), inserted('s'));
            }
        }
    }

    #[test]
    fn keys_after_alt_bracket_that_are_not_the_answer_are_typed() {
        // Nothing of an answer came: the key is typed at once.
        let mut mode = editing();
        assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
        assert_eq!(mode.parse_event(typed('l')), inserted('l'));
        assert_eq!(mode.parse_event(typed('1')), inserted('1'));

        // What read like the start of an answer was typed after all, and it is given back
        // with the key that showed it.
        let mut mode = editing();
        assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
        for character in "11;".chars() {
            assert_eq!(mode.parse_event(typed(character)), ReedlineEvent::None);
        }
        assert_eq!(
            mode.parse_event(typed('x')),
            given_back("11;", inserted('x'))
        );
        assert_eq!(mode.parse_event(typed('1')), inserted('1'));

        // Enter runs the line with it.
        let mut mode = editing();
        let enter = || key(KeyCode::Enter, KeyModifiers::NONE);
        let entered = Emacs::new(default_emacs_keybindings()).parse_event(enter());
        assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
        assert_eq!(mode.parse_event(typed('1')), ReedlineEvent::None);
        assert_eq!(mode.parse_event(enter()), given_back("1", entered));
    }

    #[test]
    fn an_answer_that_ends_before_it_names_a_colour_is_given_back() {
        for (code, modifiers) in ends() {
            let mut mode = editing();
            let ended = Emacs::new(default_emacs_keybindings()).parse_event(key(code, modifiers));
            assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
            for character in "11;rgb:14/13".chars() {
                assert_eq!(mode.parse_event(typed(character)), ReedlineEvent::None);
            }
            assert_eq!(
                mode.parse_event(key(code, modifiers)),
                given_back("11;rgb:14/13", ended)
            );
        }
    }

    #[test]
    fn an_answer_is_followed_through_what_is_not_a_key() {
        let mut mode = editing();
        let resized = || ReedlineRawEvent::try_from(Event::Resize(80, 24)).unwrap();
        assert_eq!(mode.parse_event(alt_bracket()), ReedlineEvent::None);
        for character in "11;rgb:1414/".chars() {
            assert_eq!(mode.parse_event(typed(character)), ReedlineEvent::None);
        }
        // The window changed size while the answer arrived.
        assert_eq!(
            mode.parse_event(resized()),
            Emacs::new(default_emacs_keybindings()).parse_event(resized())
        );
        for character in "1313/1b1b".chars() {
            assert_eq!(mode.parse_event(typed(character)), ReedlineEvent::None);
        }
        let (code, modifiers) = ends()[0];
        assert_eq!(mode.parse_event(key(code, modifiers)), ReedlineEvent::None);
        assert_eq!(mode.parse_event(typed('l')), inserted('l'));
    }
}

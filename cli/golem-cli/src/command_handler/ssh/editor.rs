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
/// what each one is. `unfinished` is the start of an answer from the terminal whose rest is
/// still to come.
pub fn build(
    palette: Option<Palette>,
    history: SessionHistory,
    completions: Completions,
    unfinished: Vec<u8>,
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
        .with_edit_mode(Box::new(
            WithoutLateAnswer::of(Emacs::new(keybindings)).after(unfinished),
        ))
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
/// Ctrl+G or Alt+`\`.
///
/// Every key goes on to `keys` as it comes, so the editor does with it what it always does,
/// in whatever state it is. What is kept is what the terminal sent since Alt+`]`, for as long
/// as it reads like an answer, and how many characters of it were typed into the line. When
/// the answer has arrived whole, those characters are taken out again. Any other key ends the
/// watch, and what was typed stays: nothing typed is lost, changed or put in another order.
struct WithoutLateAnswer<M> {
    keys: M,
    /// The answer that may be arriving, as the terminal sent it. Empty when none is.
    arriving: Vec<u8>,
    /// How many characters of it were typed into the line.
    typed: usize,
}

impl<M: EditMode> WithoutLateAnswer<M> {
    fn of(keys: M) -> Self {
        Self {
            keys,
            arriving: Vec::new(),
            typed: 0,
        }
    }

    /// The same, when `unfinished` of an answer has already come from the terminal.
    fn after(mut self, unfinished: Vec<u8>) -> Self {
        self.arriving = unfinished;
        self
    }

    fn pass(&mut self, event: Event) -> ReedlineEvent {
        ReedlineRawEvent::try_from(event)
            .map_or(ReedlineEvent::None, |event| self.keys.parse_event(event))
    }

    /// What the terminal sent for a key that can be part of an answer. Only `ESC ]` opens one.
    fn sent(&self, event: &Event) -> Option<Vec<u8>> {
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        else {
            return None;
        };
        match (*code, *modifiers) {
            (KeyCode::Char(']'), KeyModifiers::ALT) => Some(b"\x1b]".to_vec()),
            _ if self.arriving.is_empty() => None,
            (KeyCode::Char('\\'), KeyModifiers::ALT) => Some(b"\x1b\\".to_vec()),
            (KeyCode::Char('g'), KeyModifiers::CONTROL) => Some(vec![0x07]),
            (KeyCode::Esc, KeyModifiers::NONE) => Some(vec![0x1b]),
            (KeyCode::Char(character), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                u8::try_from(character).ok().map(|byte| vec![byte])
            }
            _ => None,
        }
    }
}

impl<M: EditMode> EditMode for WithoutLateAnswer<M> {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let event = Event::from(event);
        if matches!(
            event,
            Event::Resize(..) | Event::FocusGained | Event::FocusLost
        ) {
            // A window that changes size says nothing about what the keys around it are.
            return self.pass(event);
        }
        let sent = self.sent(&event);
        let mut arriving = std::mem::take(&mut self.arriving);
        let typed = std::mem::take(&mut self.typed);
        let Some(sent) = sent else {
            return self.pass(event);
        };
        arriving.extend_from_slice(&sent);
        match look::arrived(&arriving) {
            // The key that ends the answer is the terminal's and is not passed on.
            look::Arrived::All if typed == 0 => ReedlineEvent::None,
            look::Arrived::All => ReedlineEvent::Edit(vec![EditCommand::Backspace; typed]),
            look::Arrived::Part => {
                let made = self.pass(event);
                let character = matches!(sent.as_slice(), [byte] if !byte.is_ascii_control());
                let inserted = matches!(
                    &made,
                    ReedlineEvent::Edit(commands)
                        if matches!(commands.as_slice(), [EditCommand::InsertChar(_)])
                );
                // A character that was not simply typed leaves the line in a state this
                // cannot follow, so the watch ends there.
                if !character || inserted {
                    self.arriving = arriving;
                    self.typed = typed + usize::from(inserted);
                }
                made
            }
            look::Arrived::Nothing => {
                // It was typed. An Alt+`]` among it may still open an answer.
                if sent == b"\x1b]" {
                    self.arriving = sent;
                }
                self.pass(event)
            }
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
        Reedline, ReedlineEvent, ReedlineRawEvent, Suggestion, ValidationResult, Validator,
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

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    /// Characters as the terminal layer reports them typed: a capital comes with Shift.
    fn typed(text: &str) -> Vec<Event> {
        text.chars()
            .map(|character| {
                let modifiers = if character.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                key(KeyCode::Char(character), modifiers)
            })
            .collect()
    }

    /// `ESC ]`, which opens a terminal's answer, as the terminal layer reads it.
    fn alt_bracket() -> Event {
        key(KeyCode::Char(']'), KeyModifiers::ALT)
    }

    /// BEL and `ESC \`, either of which ends the answer, as the terminal layer reads them.
    fn ends() -> [Event; 2] {
        [
            key(KeyCode::Char('g'), KeyModifiers::CONTROL),
            key(KeyCode::Char('\\'), KeyModifiers::ALT),
        ]
    }

    /// The line after the editor took `events` through `mode`, starting from `line`, and
    /// what it was told to do besides editing the line.
    fn line_after(
        mode: &mut impl EditMode,
        line: &str,
        events: impl IntoIterator<Item = Event>,
    ) -> (String, Vec<ReedlineEvent>) {
        let mut editor = Reedline::create();
        editor.run_edit_commands(&[EditCommand::InsertString(line.to_string())]);
        let mut others = Vec::new();
        for event in events {
            match mode.parse_event(ReedlineRawEvent::try_from(event).unwrap()) {
                ReedlineEvent::Edit(commands) => editor.run_edit_commands(&commands),
                ReedlineEvent::None => {}
                other => others.push(other),
            }
        }
        (editor.current_buffer_contents().to_string(), others)
    }

    /// What `mode` makes of each of `events`.
    fn made_of(mode: &mut impl EditMode, events: &[Event]) -> Vec<ReedlineEvent> {
        events
            .iter()
            .map(|event| mode.parse_event(ReedlineRawEvent::try_from(event.clone()).unwrap()))
            .collect()
    }

    #[test]
    fn a_late_answer_about_the_background_is_taken_out_of_the_line() {
        for answer in [
            "11;rgb:1414/1313/1b1b",
            "11;rgba:1e/1e/1e/ff",
            "11;rgb:FFFF/0/8",
        ] {
            for end in ends() {
                // It arrives in the middle of a word, and the word is typed as always.
                let mut events = typed("l");
                events.push(alt_bracket());
                events.extend(typed(answer));
                events.push(end);
                events.extend(typed("s"));
                assert_eq!(
                    line_after(&mut editing(), "echo ", events),
                    ("echo ls".to_string(), vec![]),
                    "{answer:?}"
                );
            }
        }
    }

    #[test]
    fn every_key_goes_to_the_editor_as_it_comes() {
        let [bel, _] = ends();
        let control = |character| key(KeyCode::Char(character), KeyModifiers::CONTROL);
        let enter = key(KeyCode::Enter, KeyModifiers::NONE);
        let mut sequences = Vec::new();
        // In a search of the history, with something typed after Alt+`]`, and then Ctrl+C.
        let mut keys = vec![control('r'), alt_bracket()];
        keys.extend(typed("1"));
        keys.push(control('c'));
        sequences.push(keys);
        // Pasted text comes after what was typed before it.
        let mut keys = vec![alt_bracket()];
        keys.extend(typed("1"));
        keys.extend([Event::Paste("P".to_string()), enter.clone()]);
        sequences.push(keys);
        // What reads like the start of an answer and is not one, entered.
        let mut keys = vec![alt_bracket()];
        keys.extend(typed("11;x"));
        keys.push(enter);
        sequences.push(keys);
        // An answer that ends before it names a colour.
        let mut keys = vec![alt_bracket()];
        keys.extend(typed("11;rgb:14/13"));
        keys.push(bel);
        sequences.push(keys);
        for keys in sequences {
            assert_eq!(
                made_of(&mut editing(), &keys),
                made_of(&mut Emacs::new(default_emacs_keybindings()), &keys),
                "{keys:?}"
            );
        }
    }

    #[test]
    fn what_is_typed_is_never_taken_out() {
        let [bel, _] = ends();
        // An answer that ends before it names a colour stays as it was typed.
        let mut events = vec![alt_bracket()];
        events.extend(typed("11;rgb:14/13"));
        events.push(bel.clone());
        assert_eq!(line_after(&mut editing(), "", events).0, "11;rgb:14/13");
        // So does one with something else in the middle of it: a key, or pasted text.
        for between in [typed("x").remove(0), Event::Paste("P".to_string())] {
            let inserted = match &between {
                Event::Paste(text) => text.clone(),
                _ => "x".to_string(),
            };
            let mut events = vec![alt_bracket()];
            events.extend(typed("11;rgb:1414/"));
            events.push(between);
            events.extend(typed("1313/1b1b"));
            events.push(bel.clone());
            assert_eq!(
                line_after(&mut editing(), "", events).0,
                format!("11;rgb:1414/{inserted}1313/1b1b")
            );
        }
    }

    #[test]
    fn a_window_that_changes_size_does_not_end_an_answer() {
        let [bel, _] = ends();
        let mut events = vec![alt_bracket()];
        events.extend(typed("11;rgb:1414/"));
        events.push(Event::Resize(80, 24));
        events.extend(typed("1313/1b1b"));
        events.push(bel);
        assert_eq!(
            line_after(&mut editing(), "ls", events),
            ("ls".to_string(), vec![ReedlineEvent::Resize(80, 24)])
        );
    }

    #[test]
    fn an_answer_that_the_wait_cut_off_is_taken_out_when_its_rest_comes() {
        let [bel, st] = ends();
        let rest = |text: &str, end: Option<&Event>| {
            let mut events = typed(text);
            events.extend(end.cloned());
            events
        };
        for (unfinished, rest) in [
            // Only the escape that opens the answer came in time.
            (&b"\x1b]"[..], rest("11;rgb:f/f/f", Some(&bel))),
            (b"\x1b", rest("]11;rgb:f/f/f", Some(&bel))),
            // It was cut in the middle of the colour, and in the middle of what ends it.
            (b"\x1b]11;rgb:14", rest("14/1313/1b1b", Some(&st))),
            (b"\x1b]11;rgb:f/f/f\x1b", rest("\\", None)),
            // The answer about the device attributes was cut.
            (b"\x1b[?6", rest("2;c", None)),
            (b"\x1b", rest("[?62;4c", None)),
        ] {
            let mut events = rest;
            events.extend(typed("ls"));
            assert_eq!(
                line_after(&mut editing().after(unfinished.to_vec()), "echo ", events),
                ("echo ls".to_string(), vec![]),
                "{unfinished:?}"
            );
        }
        // The rest never came: what is typed is typed.
        assert_eq!(
            line_after(&mut editing().after(b"\x1b]".to_vec()), "", typed("ls")).0,
            "ls"
        );
    }
}

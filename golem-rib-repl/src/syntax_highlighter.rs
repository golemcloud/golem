use rustyline::highlight::Highlighter;
use rustyline::config::CompletionType;
use rustyline::highlight::CmdKind;
use std::borrow::Cow;
use rustyline::completion::Completer;
use rustyline::Helper;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use termion::color;

#[derive(Default)]
pub struct RibSyntaxHighlighter;

impl Helper for RibSyntaxHighlighter {}

impl Completer for RibSyntaxHighlighter {
    type Candidate = String;
}

impl Hinter for RibSyntaxHighlighter {
    type Hint = String;
}

impl Validator for RibSyntaxHighlighter {}

impl Highlighter for RibSyntaxHighlighter {
    /// Highlights Rib code in the REPL
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut highlighted = String::new();

        for word in line.split_whitespace() {
            if [
                "let", "fn", "if", "else", "match", "return", "while", "for", "in",
                "break", "continue", "true", "yield", "false", "some", "none", "ok", "error",
            ].contains(&word) {
                // Highlight keywords in blue
                highlighted.push_str(&format!(
                    "{}{}{} ",
                    color::Fg(color::Blue),
                    word,
                    color::Fg(color::Reset)
                ));
            } else if word.starts_with("\"") && word.ends_with("\"") {
                // Highlight strings in green
                highlighted.push_str(&format!(
                    "{}{}{} ",
                    color::Fg(color::Green),
                    word,
                    color::Fg(color::Reset)
                ));
            } else if word.chars().all(|c| c.is_numeric()) {
                // Highlight numbers in yellow
                highlighted.push_str(&format!(
                    "{}{}{} ",
                    color::Fg(color::Yellow),
                    word,
                    color::Fg(color::Reset)
                ));
            } else if word.starts_with(".") {
                // Highlight field selections and method calls in cyan
                highlighted.push_str(&format!(
                    "{}{}{} ",
                    color::Fg(color::Cyan),
                    word,
                    color::Fg(color::Reset)
                ));
            } else if [">=", "<=", "==", "<", ">", "&&", "||", "+", "-", "*", "/"].contains(&word) {
                // Highlight operators in magenta
                highlighted.push_str(&format!(
                    "{}{}{} ",
                    color::Fg(color::Magenta),
                    word,
                    color::Fg(color::Reset)
                ));
            } else {
                highlighted.push_str(&format!("{} ", word));
            }
        }

        Cow::Owned(highlighted.trim_end().to_string())
    }

    /// Highlights the REPL prompt (can be customized)
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Owned(format!("{}{}{}", color::Fg(color::Cyan), prompt, color::Fg(color::Reset)))
    }

    /// Highlights hints (optional)
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("{}{}{}", color::Fg(color::LightBlack), hint, color::Fg(color::Reset)))
    }

    /// Highlights autocompletion candidates (optional)
    fn highlight_candidate<'c>(&self, candidate: &'c str, _completion: CompletionType) -> Cow<'c, str> {
        Cow::Owned(format!("{}{}{}", color::Fg(color::Magenta), candidate, color::Fg(color::Reset)))
    }

    /// Defines if highlighting should change when cursor moves
    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        true
    }
}

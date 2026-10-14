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

//! A small bash lexer for the prompt: enough to tell whether typed input is finished, to colour
//! it, and to find the word under the cursor. It is not a parser. Where it cannot tell, it says
//! the input is finished, so the text is sent and bash reports any error.

/// What a stretch of the input is, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Unquoted text of a word. `command` is set when the whole word is a command name.
    Word {
        command: bool,
    },
    /// A reserved word where bash reads it as one (`if`, `then`, `done`, ...).
    Reserved,
    /// Quoted text, quotes included.
    Quoted,
    /// `$name`, `${...}`, arithmetic, and the delimiters of `$(...)` and backquotes.
    Expansion,
    /// `|`, `&&`, `;`, `(`, `)` and the like.
    Operator,
    /// `>`, `>>`, `<`, `<<`, `2>&1` and the like.
    Redirect,
    Comment,
    /// The body of a here-document, terminator line included.
    Heredoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The tokens in order. Whitespace and unrecognised bytes are in no token.
    pub tokens: Vec<Token>,
    /// False only when the input is certainly unfinished.
    pub complete: bool,
    /// A word starting at the end of the input would be a command name.
    pub command_next: bool,
    /// The end of the input is outside every quote, expansion, comment and here-document.
    pub plain_end: bool,
    /// The last word token is a later part of a word that also has a quote or an expansion.
    pub word_fragment: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Command,
    Argument,
}

/// The unquoted word that ends at the cursor; empty when the cursor is where a word would start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorWord {
    pub start: usize,
    pub text: String,
    pub position: Position,
}

pub fn scan(input: &str) -> Scan {
    let mut scanner = Scanner::new(input);
    scanner.run();
    scanner.finish()
}

/// Whether bash reads `word` as a reserved word where a command starts.
pub fn is_reserved_word(word: &str) -> bool {
    matches!(
        scan(word).tokens.as_slice(),
        [Token { kind: TokenKind::Reserved, start: 0, end }] if *end == word.len()
    )
}

/// Whether Enter should submit `input`, rather than continue it on a new line.
pub fn is_complete(input: &str) -> bool {
    scan(input).complete
}

/// The word a completion at `cursor` would extend, as the name it stands for. A completion
/// writes a backslash before a character that means something to bash and `$'...'` around one
/// that would act on the terminal, and both are read back here, so that a completed path can be
/// completed further. `None` inside quotes, expansions, comments and here-documents, and for
/// words with other quoting, an unfinished escape, globs or a leading `~`.
pub fn cursor_word(input: &str, cursor: usize) -> Option<CursorWord> {
    let prefix = input.get(..cursor)?;
    let scan = scan(prefix);
    if !scan.plain_end {
        return None;
    }
    // The word's parts, back from the cursor: each ends where the one after it starts.
    let mut start = cursor;
    let mut parts = Vec::new();
    let mut command = false;
    let mut spelled_out = false;
    // The word runs into something on its left that is not read back.
    let mut joined = false;
    for &Token {
        kind,
        start: from,
        end,
    } in scan.tokens.iter().rev()
    {
        if end != start {
            break;
        }
        let text = &prefix[from..end];
        let part = match kind {
            TokenKind::Word { command: whole } => {
                command = whole;
                unescaped(text)
            }
            // A reserved word may be the start of a longer command name: `fi` of `find`.
            TokenKind::Reserved => {
                command = true;
                unescaped(text)
            }
            TokenKind::Quoted => {
                spelled_out = true;
                spelled_out_text(text)
            }
            // The word starts after an operator or a redirection.
            TokenKind::Operator | TokenKind::Redirect => break,
            _ => None,
        };
        let Some(part) = part else {
            joined = !parts.is_empty();
            break;
        };
        parts.push(part);
        start = from;
    }
    if !parts.is_empty() {
        if prefix[start..].starts_with('~') {
            return None;
        }
        // One unquoted part is a word of its own unless the lexer saw it continue another. A
        // word in several parts has to start where a word can.
        let whole = if parts.len() == 1 && !spelled_out {
            !scan.word_fragment
        } else {
            command = false;
            !joined
                && prefix[..start]
                    .bytes()
                    .next_back()
                    .is_none_or(|byte| byte.is_ascii_whitespace() || is_meta(byte))
        };
        return whole.then(|| CursorWord {
            start,
            text: parts.into_iter().rev().collect(),
            position: if command {
                Position::Command
            } else {
                Position::Argument
            },
        });
    }
    let at_word_start = prefix
        .as_bytes()
        .last()
        .is_none_or(|&byte| byte.is_ascii_whitespace() || is_meta(byte));
    at_word_start.then(|| CursorWord {
        start: cursor,
        text: String::new(),
        position: if scan.command_next {
            Position::Command
        } else {
            Position::Argument
        },
    })
}

/// An open construct that must be closed before the input is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// `(`. `substitution` marks `$(`, `<(` and `>(`, which continue a word when they close;
    /// `outer_expect` is what the grammar expected after the word such a one is part of.
    Paren {
        substitution: bool,
        outer_command: bool,
        outer_expect: Expect,
    },
    /// `((` or `$((`, with the number of parentheses still open.
    Arithmetic {
        depth: u32,
        substitution: bool,
    },
    /// `${`
    Parameter,
    DoubleQuote,
    /// A backquote with no closing backquote after it. `unsure` is as it was before it: bash
    /// does not parse what a backquote holds until it runs, so nothing in there is an error yet.
    Backquote {
        unsure: bool,
    },
    If,
    /// `while`, `until`, `for` or `select`, closed by `done`.
    Loop,
    /// `pattern` is set while reading the patterns before a `)`.
    Case {
        pattern: bool,
    },
    /// `{ ...; }`
    Group,
    /// `[[ ... ]]`
    Condition,
    /// The subscript of `name[...]` where bash reads one, with the number of `[` still open.
    Subscript {
        depth: u32,
    },
}

/// What the grammar requires of the next word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Nothing,
    /// The name after `function`.
    FunctionName,
    /// The variable after `for` or `select`, or the subject after `case`.
    Subject {
        case: bool,
    },
    /// `in` (or `do`) after that variable or subject.
    In {
        case: bool,
    },
    /// The target of a redirection.
    Target,
    /// Nothing, but an assignment, a redirection or `time` has come before the command name.
    Prefixed,
}

/// The text an unquoted part of a word stands for: a backslash makes the character after it
/// plain. `None` for a glob character and for a backslash that ends the part or the line.
fn unescaped(text: &str) -> Option<String> {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next()? {
                '\n' => return None,
                escaped => plain.push(escaped),
            },
            '*' | '?' | '[' | '{' => return None,
            _ => plain.push(character),
        }
    }
    Some(plain)
}

/// The text `$'...'` stands for, with the escapes a completion writes in it: `\e`, `\n`, `\t`,
/// `\xHH`, `\uHHHH` and `\UHHHHHHHH`. `None` for any other escape and any other quoting.
fn spelled_out_text(text: &str) -> Option<String> {
    let mut rest = text.strip_prefix("$'")?.strip_suffix('\'')?;
    let mut plain = String::with_capacity(rest.len());
    while let Some(character) = rest.chars().next() {
        rest = &rest[character.len_utf8()..];
        if character != '\\' {
            plain.push(character);
            continue;
        }
        let escape = rest.chars().next()?;
        rest = &rest[escape.len_utf8()..];
        let digits = match escape {
            'e' => {
                plain.push('\x1b');
                continue;
            }
            'n' => {
                plain.push('\n');
                continue;
            }
            't' => {
                plain.push('\t');
                continue;
            }
            'x' => 2,
            'u' => 4,
            'U' => 8,
            _ => return None,
        };
        let code = rest.get(..digits)?;
        if !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        plain.push(char::from_u32(u32::from_str_radix(code, 16).ok()?)?);
        rest = &rest[digits..];
    }
    Some(plain)
}

fn is_meta(byte: u8) -> bool {
    matches!(byte, b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>')
}

/// `NAME=`, `NAME+=` or `NAME[...]=` at the start of a word.
fn is_assignment(text: &str) -> bool {
    let Some(equals) = text.find('=') else {
        return false;
    };
    let name = text[..equals].strip_suffix('+').unwrap_or(&text[..equals]);
    let name = match (name.find('['), name.ends_with(']')) {
        (Some(bracket), true) => &name[..bracket],
        _ => name,
    };
    is_name(name)
}

/// A variable name.
fn is_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `name[` and what follows, when the `]` that matches that `[` is still to come: `a[`,
/// `a[b[c]`.
fn opens_subscript(text: &str) -> bool {
    let Some(bracket) = text.find('[') else {
        return false;
    };
    let mut depth = 0;
    for byte in text[bracket..].bytes() {
        match byte {
            b'[' => depth += 1,
            b']' if depth == 1 => return false,
            b']' => depth -= 1,
            _ => {}
        }
    }
    is_name(&text[..bracket])
}

struct Scanner<'a> {
    text: &'a str,
    src: &'a [u8],
    i: usize,
    tokens: Vec<Token>,
    open: Vec<Open>,
    /// How many of the open constructs are double quotes.
    double_quotes: usize,
    /// Here-documents whose bodies start after the next newline: terminator, and `<<-`.
    heredocs: Vec<(Vec<u8>, bool)>,
    /// The next word is in command position.
    command: bool,
    expect: Expect,
    /// The previous byte closed a quote or an expansion, so text here continues that word.
    continues_word: bool,
    /// The word being scanned already has a quote or an expansion before this point.
    word_fragment: bool,
    /// Something does not fit the grammar; let bash report it.
    unsure: bool,
    /// A quote or a here-document runs to the end of the input.
    unterminated: bool,
    trailing_backslash: bool,
    /// The last operator was `|`, `&&` or `||`, and no command has followed it.
    awaits_command: bool,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            src: text.as_bytes(),
            i: 0,
            tokens: Vec::new(),
            open: Vec::new(),
            double_quotes: 0,
            heredocs: Vec::new(),
            command: true,
            expect: Expect::Nothing,
            continues_word: false,
            word_fragment: false,
            unsure: false,
            unterminated: false,
            trailing_backslash: false,
            awaits_command: false,
        }
    }

    fn run(&mut self) {
        while self.i < self.src.len() {
            let before = (self.i, self.open.len());
            match self.open.last() {
                Some(Open::DoubleQuote) => self.double_quote(),
                Some(Open::Parameter) => self.parameter(),
                Some(Open::Arithmetic { .. }) => self.arithmetic(),
                Some(Open::Subscript { .. }) => self.subscript(),
                _ => self.command_mode(),
            }
            // Every pass consumes input or opens or closes a construct. Should one ever do
            // neither, skip a character rather than scan it forever.
            if (self.i, self.open.len()) == before {
                self.i += 1;
                while !self.text.is_char_boundary(self.i) {
                    self.i += 1;
                }
            }
        }
    }

    fn finish(self) -> Scan {
        let unfinished = !self.open.is_empty()
            || self.unterminated
            || !self.heredocs.is_empty()
            || self.trailing_backslash
            || self.awaits_command;
        // Only what came before an unclosed backquote can be an error for bash to report now.
        let unsure = self
            .open
            .iter()
            .find_map(|open| match open {
                Open::Backquote { unsure } => Some(*unsure),
                _ => None,
            })
            .unwrap_or(self.unsure);
        let in_comment = self
            .tokens
            .last()
            .is_some_and(|token| token.kind == TokenKind::Comment && token.end == self.src.len());
        let plain_end = !self.unterminated
            && !in_comment
            && !matches!(
                self.open.last(),
                Some(
                    Open::DoubleQuote
                        | Open::Parameter
                        | Open::Arithmetic { .. }
                        | Open::Subscript { .. }
                )
            );
        let command_next = plain_end
            && !self.continues_word
            && matches!(self.expect, Expect::Nothing | Expect::Prefixed)
            && self.command
            && !self.reads_plain_words();
        Scan {
            tokens: self.tokens,
            complete: unsure || !unfinished,
            command_next,
            plain_end,
            word_fragment: self.word_fragment,
        }
    }

    fn token(&mut self, kind: TokenKind, start: usize, end: usize) {
        if end > start {
            self.tokens.push(Token { kind, start, end });
        }
    }

    fn top_is(&self, open: Open) -> bool {
        self.open.last() == Some(&open)
    }

    /// Case patterns and `[[ ]]` hold words, never commands.
    fn reads_plain_words(&self) -> bool {
        matches!(
            self.open.last(),
            Some(Open::Case { pattern: true } | Open::Condition)
        )
    }

    fn set_case_pattern(&mut self, reading: bool) {
        if let Some(Open::Case { pattern }) = self.open.last_mut() {
            *pattern = reading;
        }
    }

    fn close(&mut self, matches_top: impl Fn(&Open) -> bool) {
        if self.open.last().is_some_and(matches_top) {
            self.open.pop();
        } else {
            self.unsure = true;
        }
    }

    fn command_mode(&mut self) {
        match self.src[self.i] {
            b'\n' => {
                self.i += 1;
                self.continues_word = false;
                self.newline();
            }
            // Any other whitespace separates words, a carriage return or a form feed included.
            byte if byte.is_ascii_whitespace() => {
                self.i += 1;
                self.continues_word = false;
            }
            b'#' if !self.continues_word => {
                let start = self.i;
                while self.i < self.src.len() && self.src[self.i] != b'\n' {
                    self.i += 1;
                }
                self.token(TokenKind::Comment, start, self.i);
            }
            b'\\' => match self.src.get(self.i + 1) {
                None => {
                    self.trailing_backslash = true;
                    self.i += 1;
                }
                // A backslash-newline joins the two lines.
                Some(b'\n') => self.i += 2,
                Some(_) => self.word(),
            },
            byte if is_meta(byte) => self.operator(),
            _ => self.word(),
        }
    }

    fn newline(&mut self) {
        if !self.heredocs.is_empty() {
            self.heredoc_bodies();
        }
        if !matches!(self.expect, Expect::In { .. }) {
            self.expect = Expect::Nothing;
            if !self.reads_plain_words() {
                self.command = true;
            }
        }
    }

    fn heredoc_bodies(&mut self) {
        let start = self.i;
        for (terminator, strip_tabs) in std::mem::take(&mut self.heredocs) {
            loop {
                if self.i >= self.src.len() {
                    self.unterminated = true;
                    break;
                }
                let end = self.src[self.i..]
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(self.src.len(), |offset| self.i + offset);
                let mut line = &self.src[self.i..end];
                while strip_tabs && line.first() == Some(&b'\t') {
                    line = &line[1..];
                }
                self.i = (end + 1).min(self.src.len());
                if line == terminator.as_slice() {
                    break;
                }
                if end == self.src.len() {
                    self.unterminated = true;
                    break;
                }
            }
            if self.unterminated {
                break;
            }
        }
        self.token(TokenKind::Heredoc, start, self.i);
    }

    fn operator(&mut self) {
        let start = self.i;
        let rest = &self.src[start..];
        self.continues_word = false;
        if matches!(rest[0], b'<' | b'>') || rest.starts_with(b"&>") {
            return self.redirect(start);
        }
        let pattern = self.top_is(Open::Case { pattern: true });
        if rest.starts_with(b";;") || rest.starts_with(b";&") {
            self.i += if rest.starts_with(b";;&") { 3 } else { 2 };
            if matches!(self.open.last(), Some(Open::Case { .. })) {
                self.set_case_pattern(true);
            } else {
                self.unsure = true;
            }
            self.command = true;
            self.awaits_command = false;
        } else if rest.starts_with(b"&&") || rest.starts_with(b"||") {
            self.i += 2;
            self.command = true;
            self.awaits_command = true;
        } else if rest[0] == b'|' {
            self.i += if rest.starts_with(b"|&") { 2 } else { 1 };
            if !pattern {
                self.command = true;
                self.awaits_command = true;
            }
        } else if rest[0] == b';' || rest[0] == b'&' {
            self.i += 1;
            self.command = true;
            self.awaits_command = false;
        } else if rest[0] == b'(' {
            let arithmetic = rest.starts_with(b"((")
                && !pattern
                && (self.command || self.expect == Expect::Subject { case: false });
            if pattern {
                self.i += 1;
            } else if arithmetic {
                self.i += 2;
                self.open.push(Open::Arithmetic {
                    depth: 2,
                    substitution: false,
                });
            } else {
                self.i += 1;
                self.open.push(Open::Paren {
                    substitution: false,
                    outer_command: self.command,
                    outer_expect: Expect::Nothing,
                });
                self.command = true;
            }
            self.awaits_command = false;
        } else {
            // `)`
            self.i += 1;
            if pattern {
                self.set_case_pattern(false);
                self.command = true;
            } else if let Some(Open::Paren {
                substitution,
                outer_command,
                outer_expect,
            }) = self.open.last().copied()
            {
                self.open.pop();
                if substitution {
                    self.command = outer_command;
                    self.continues_word = true;
                    self.expect = outer_expect;
                    return self.token(TokenKind::Expansion, start, self.i);
                }
                self.command = true;
            } else {
                self.unsure = true;
            }
        }
        self.expect = Expect::Nothing;
        self.token(TokenKind::Operator, start, self.i);
    }

    /// A redirection whose token starts at `start`; `self.i` is at its `<`, `>` or `&>`.
    fn redirect(&mut self, start: usize) {
        self.awaits_command = false;
        let rest = &self.src[self.i..];
        if rest.starts_with(b"<(") || rest.starts_with(b">(") {
            self.i += 2;
            let outer_expect = match std::mem::replace(&mut self.expect, Expect::Nothing) {
                // Before the command name this counts as a redirection.
                Expect::Nothing if self.command => Expect::Prefixed,
                expect => expect,
            };
            self.open.push(Open::Paren {
                substitution: true,
                outer_command: self.command,
                outer_expect,
            });
            self.command = true;
            return self.token(TokenKind::Expansion, start, self.i);
        }
        if rest.starts_with(b"<<") && !rest.starts_with(b"<<<") {
            let strip_tabs = rest.starts_with(b"<<-");
            self.i += if strip_tabs { 3 } else { 2 };
            self.token(TokenKind::Redirect, start, self.i);
            return self.heredoc_terminator(strip_tabs);
        }
        self.i += [
            &b"<<<"[..],
            b"&>>",
            b"<&",
            b"<>",
            b">>",
            b">&",
            b">|",
            b"&>",
        ]
        .iter()
        .find(|operator| rest.starts_with(operator))
        .map_or(1, |operator| operator.len());
        self.expect = Expect::Target;
        self.token(TokenKind::Redirect, start, self.i);
    }

    fn heredoc_terminator(&mut self, strip_tabs: bool) {
        while matches!(self.src.get(self.i), Some(b' ' | b'\t')) {
            self.i += 1;
        }
        let start = self.i;
        let mut terminator = Vec::new();
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                b' ' | b'\t' | b'\n' => break,
                byte if is_meta(byte) => break,
                b'\'' | b'"' => {
                    let Some(length) = self.src[self.i + 1..]
                        .iter()
                        .position(|&other| other == byte)
                    else {
                        self.unterminated = true;
                        self.i = self.src.len();
                        break;
                    };
                    terminator.extend_from_slice(&self.src[self.i + 1..self.i + 1 + length]);
                    self.i += length + 2;
                }
                b'\\' => {
                    terminator.extend(self.src.get(self.i + 1));
                    self.i = (self.i + 2).min(self.src.len());
                }
                byte => {
                    terminator.push(byte);
                    self.i += 1;
                }
            }
        }
        if self.i == start {
            // `<<` with no word after it is an error bash reports.
            self.unsure = true;
            return;
        }
        self.token(TokenKind::Word { command: false }, start, self.i);
        if self.command && self.expect == Expect::Nothing {
            self.expect = Expect::Prefixed;
        }
        if !self.unterminated {
            self.heredocs.push((terminator, strip_tabs));
        }
    }

    /// A word starts at `self.i`.
    fn word(&mut self) {
        let start = self.i;
        self.word_fragment = self.continues_word;
        if self.continues_word {
            return self.segments(start, TokenKind::Word { command: false });
        }
        let mut plain_end = start;
        while let Some(&byte) = self.src.get(plain_end) {
            if byte.is_ascii_whitespace()
                || is_meta(byte)
                || matches!(byte, b'\\' | b'\'' | b'"' | b'$' | b'`')
            {
                break;
            }
            plain_end += 1;
        }
        let text = &self.text[start..plain_end];
        let next = self.src.get(plain_end).copied();
        // `2>file`: the digits belong to the redirection.
        if matches!(next, Some(b'<' | b'>'))
            && !text.is_empty()
            && text.bytes().all(|byte| byte.is_ascii_digit())
        {
            self.i = plain_end;
            self.continues_word = false;
            return self.redirect(start);
        }
        let whole =
            !text.is_empty() && next.is_none_or(|byte| byte.is_ascii_whitespace() || is_meta(byte));
        self.awaits_command = false;
        // At the start of a command bash reads `name[` up to the matching `]` as part of the
        // word, blanks and operators included. After an assignment, a redirection or `time`,
        // and in parentheses, which may hold an array's words rather than commands, whether it
        // does depends on more than is kept here, so there `[` stays an ordinary character.
        let subscript = self.command
            && self.expect == Expect::Nothing
            && !self.reads_plain_words()
            && !matches!(
                self.open.last(),
                Some(Open::Paren {
                    substitution: false,
                    ..
                })
            )
            && opens_subscript(text);
        if subscript {
            // Scanned from the name on, as one word with what its brackets hold.
            self.classify(None, text);
            self.open.push(Open::Subscript { depth: 0 });
            return;
        }
        let kind = self.classify(whole.then_some(text), text);
        self.segments(start, kind);
    }

    /// Decides what the word starting here is and moves the grammar past it. `whole` is the
    /// word when it is plain text only; `leading` is its plain text before any quote.
    fn classify(&mut self, whole: Option<&str>, leading: &str) -> TokenKind {
        let plain = TokenKind::Word { command: false };
        match self.expect {
            Expect::Nothing | Expect::Prefixed => {}
            Expect::Target => {
                self.expect = if self.command {
                    Expect::Prefixed
                } else {
                    Expect::Nothing
                };
                return plain;
            }
            Expect::FunctionName => {
                self.expect = Expect::Nothing;
                self.command = true;
                return plain;
            }
            Expect::Subject { case } => {
                self.expect = Expect::In { case };
                self.command = false;
                return plain;
            }
            Expect::In { case } => {
                self.expect = Expect::Nothing;
                return match whole {
                    Some("in") => {
                        self.set_case_pattern(case);
                        self.command = false;
                        TokenKind::Reserved
                    }
                    Some("do") if !case => {
                        self.command = true;
                        TokenKind::Reserved
                    }
                    _ => plain,
                };
            }
        }
        if self.top_is(Open::Case { pattern: true }) {
            if whole == Some("esac") {
                self.open.pop();
                self.command = false;
                return TokenKind::Reserved;
            }
            return plain;
        }
        if self.top_is(Open::Condition) {
            if whole == Some("]]") {
                self.open.pop();
                return TokenKind::Reserved;
            }
            return plain;
        }
        if !self.command {
            return plain;
        }
        if let Some(word) = whole
            && self.reserved(word)
        {
            return TokenKind::Reserved;
        }
        if is_assignment(leading) {
            // An assignment before a command leaves the command position open.
            self.expect = Expect::Prefixed;
            return plain;
        }
        self.command = false;
        TokenKind::Word {
            command: whole.is_some(),
        }
    }

    /// Applies a reserved word in command position; false when `word` is not one.
    fn reserved(&mut self, word: &str) -> bool {
        match word {
            "if" => {
                self.open.push(Open::If);
                self.command = true;
            }
            "then" | "else" | "elif" | "do" | "!" => self.command = true,
            "time" => {
                self.command = true;
                // After `|` bash takes `time` for the command name, not for a reserved word.
                self.expect = Expect::Prefixed;
            }
            "fi" => {
                self.close(|open| *open == Open::If);
                self.command = false;
            }
            "while" | "until" => {
                self.open.push(Open::Loop);
                self.command = true;
            }
            "for" | "select" => {
                self.open.push(Open::Loop);
                self.expect = Expect::Subject { case: false };
                self.command = false;
            }
            "done" => {
                self.close(|open| *open == Open::Loop);
                self.command = false;
            }
            "case" => {
                self.open.push(Open::Case { pattern: false });
                self.expect = Expect::Subject { case: true };
                self.command = false;
            }
            "esac" => {
                self.close(|open| matches!(open, Open::Case { .. }));
                self.command = false;
            }
            "function" => {
                self.expect = Expect::FunctionName;
                self.command = false;
            }
            "{" => {
                self.open.push(Open::Group);
                self.command = true;
            }
            "}" => {
                self.close(|open| *open == Open::Group);
                self.command = false;
            }
            "[[" => {
                self.open.push(Open::Condition);
                self.command = false;
            }
            _ => return false,
        }
        true
    }

    /// Scans the word from `start` to its end, or to the point where a nested construct opens.
    fn segments(&mut self, start: usize, kind: TokenKind) {
        let mut run = start;
        self.i = start;
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                byte if byte.is_ascii_whitespace() || is_meta(byte) => break,
                b'\\' => match self.src.get(self.i + 1) {
                    None => {
                        self.trailing_backslash = true;
                        self.i += 1;
                    }
                    Some(_) => self.i += 2,
                },
                b'\'' => {
                    self.token(kind, run, self.i);
                    let quote = self.i;
                    self.i = match self.src[quote + 1..].iter().position(|&b| b == b'\'') {
                        Some(length) => quote + length + 2,
                        None => {
                            self.unterminated = true;
                            self.src.len()
                        }
                    };
                    self.token(TokenKind::Quoted, quote, self.i);
                    self.word_fragment = true;
                    run = self.i;
                }
                b'"' => {
                    self.token(kind, run, self.i);
                    self.token(TokenKind::Quoted, self.i, self.i + 1);
                    self.i += 1;
                    self.open.push(Open::DoubleQuote);
                    self.double_quotes += 1;
                    return;
                }
                b'`' => {
                    self.token(kind, run, self.i);
                    self.token(TokenKind::Expansion, self.i, self.i + 1);
                    self.i += 1;
                    self.backquote();
                    return;
                }
                b'$' => {
                    self.token(kind, run, self.i);
                    run = self.i;
                    match self.dollar(false) {
                        Dollar::Opened => return,
                        Dollar::Consumed => {
                            self.word_fragment = true;
                            run = self.i;
                        }
                        Dollar::Literal => {}
                    }
                }
                _ => self.i += 1,
            }
        }
        self.token(kind, run, self.i);
    }

    /// Handles the `$` at `self.i`.
    fn dollar(&mut self, in_double_quote: bool) -> Dollar {
        let start = self.i;
        let next = self.src.get(start + 1).copied();
        match next {
            Some(b'(') if self.src.get(start + 2) == Some(&b'(') => {
                self.i += 3;
                self.open.push(Open::Arithmetic {
                    depth: 2,
                    substitution: true,
                });
            }
            Some(b'(') => {
                self.i += 2;
                let outer_expect = std::mem::replace(&mut self.expect, Expect::Nothing);
                self.open.push(Open::Paren {
                    substitution: true,
                    outer_command: self.command,
                    outer_expect,
                });
                self.command = true;
            }
            Some(b'{') => {
                self.i += 2;
                self.open.push(Open::Parameter);
            }
            Some(b'\'') if !in_double_quote => {
                // $'...' with backslash escapes
                self.i += 2;
                loop {
                    match self.src.get(self.i) {
                        None => {
                            self.unterminated = true;
                            break;
                        }
                        Some(b'\\') => self.i = (self.i + 2).min(self.src.len()),
                        Some(b'\'') => {
                            self.i += 1;
                            break;
                        }
                        Some(_) => self.i += 1,
                    }
                }
                self.token(TokenKind::Quoted, start, self.i);
                return Dollar::Consumed;
            }
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'_' => {
                self.i += 2;
                while self
                    .src
                    .get(self.i)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    self.i += 1;
                }
                self.token(TokenKind::Expansion, start, self.i);
                return Dollar::Consumed;
            }
            Some(byte) if byte.is_ascii_digit() || b"?@*#$!-".contains(&byte) => {
                self.i += 2;
                self.token(TokenKind::Expansion, start, self.i);
                return Dollar::Consumed;
            }
            _ => {
                self.i += 1;
                return Dollar::Literal;
            }
        }
        self.token(TokenKind::Expansion, start, self.i);
        Dollar::Opened
    }

    /// Reads a backquote substitution; `self.i` is just past its opening backquote.
    fn backquote(&mut self) {
        // Bash does not parse the text of the substitution until it runs it: the next backquote
        // with no backslash before it closes the substitution, whatever was opened in between.
        let mut close = self.i;
        while self.src.get(close).is_some_and(|&byte| byte != b'`') {
            close += if self.src[close] == b'\\' { 2 } else { 1 };
        }
        if close >= self.src.len() {
            // No closing backquote yet: the rest of the input is that text.
            self.open.push(Open::Backquote {
                unsure: self.unsure,
            });
            self.expect = Expect::Nothing;
            self.command = true;
            return;
        }
        // Nothing the text leaves open reaches past the closing backquote, so it is scanned on
        // its own, as input that ends there, and only its tokens are kept.
        let mut inside = Scanner::new(&self.text[..close]);
        inside.i = self.i;
        inside.tokens = std::mem::take(&mut self.tokens);
        inside.run();
        self.tokens = inside.tokens;
        self.token(TokenKind::Expansion, close, close + 1);
        self.i = close + 1;
        self.continues_word = true;
    }

    fn double_quote(&mut self) {
        let mut run = self.i;
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                b'"' => {
                    self.i += 1;
                    self.token(TokenKind::Quoted, run, self.i);
                    self.open.pop();
                    self.double_quotes -= 1;
                    self.continues_word = true;
                    return;
                }
                b'\\' => self.i = (self.i + 2).min(self.src.len()),
                b'$' => {
                    self.token(TokenKind::Quoted, run, self.i);
                    run = self.i;
                    match self.dollar(true) {
                        Dollar::Opened => return,
                        Dollar::Consumed => run = self.i,
                        Dollar::Literal => {}
                    }
                }
                b'`' => {
                    self.token(TokenKind::Quoted, run, self.i);
                    self.token(TokenKind::Expansion, self.i, self.i + 1);
                    self.i += 1;
                    self.backquote();
                    return;
                }
                _ => self.i += 1,
            }
        }
        self.token(TokenKind::Quoted, run, self.i);
    }

    fn parameter(&mut self) {
        let start = self.i;
        let quoted = self.double_quotes > 0;
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                b'}' => {
                    self.i += 1;
                    self.token(TokenKind::Expansion, start, self.i);
                    self.open.pop();
                    self.continues_word = true;
                    return;
                }
                b'\\' => self.i = (self.i + 2).min(self.src.len()),
                // Inside double quotes a single quote here is an ordinary character.
                b'\'' if !quoted => {
                    self.i = match self.src[self.i + 1..].iter().position(|&b| b == b'\'') {
                        Some(length) => self.i + length + 2,
                        None => self.src.len(),
                    };
                }
                b'"' => {
                    self.token(TokenKind::Expansion, start, self.i);
                    self.token(TokenKind::Quoted, self.i, self.i + 1);
                    self.i += 1;
                    self.open.push(Open::DoubleQuote);
                    self.double_quotes += 1;
                    return;
                }
                b'`' => {
                    self.token(TokenKind::Expansion, start, self.i + 1);
                    self.i += 1;
                    self.backquote();
                    return;
                }
                b'$' if matches!(self.src.get(self.i + 1), Some(b'(' | b'{')) => {
                    self.token(TokenKind::Expansion, start, self.i);
                    self.dollar(true);
                    return;
                }
                _ => self.i += 1,
            }
        }
        self.token(TokenKind::Expansion, start, self.i);
    }

    fn arithmetic(&mut self) {
        let start = self.i;
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                b'(' => {
                    self.i += 1;
                    if let Some(Open::Arithmetic { depth, .. }) = self.open.last_mut() {
                        *depth += 1;
                    }
                }
                b')' => {
                    self.i += 1;
                    let Some(Open::Arithmetic {
                        depth,
                        substitution,
                    }) = self.open.last_mut()
                    else {
                        unreachable!("arithmetic is scanned only while it is the top")
                    };
                    *depth -= 1;
                    if *depth == 0 {
                        let substitution = *substitution;
                        self.open.pop();
                        self.token(TokenKind::Expansion, start, self.i);
                        if substitution {
                            self.continues_word = true;
                        } else {
                            self.command = false;
                        }
                        return;
                    }
                }
                b'$' if self.src.get(self.i + 1) == Some(&b'(')
                    && self.src.get(self.i + 2) != Some(&b'(') =>
                {
                    self.token(TokenKind::Expansion, start, self.i);
                    self.dollar(true);
                    return;
                }
                b'\'' | b'"' => {
                    self.i = match self.src[self.i + 1..].iter().position(|&b| b == byte) {
                        Some(length) => self.i + length + 2,
                        None => self.src.len(),
                    };
                }
                _ => self.i += 1,
            }
        }
        self.token(TokenKind::Expansion, start, self.i);
    }

    /// A name and its subscript, up to the `]` that matches the first `[`. Bash reads it all
    /// as part of one word: blanks, operators and reserved words mean nothing in it.
    fn subscript(&mut self) {
        let word = TokenKind::Word { command: false };
        let mut run = self.i;
        while let Some(&byte) = self.src.get(self.i) {
            match byte {
                b'[' | b']' => {
                    self.i += 1;
                    let Some(Open::Subscript { depth }) = self.open.last_mut() else {
                        unreachable!("a subscript is scanned only while it is the top")
                    };
                    *depth = if byte == b'[' { *depth + 1 } else { *depth - 1 };
                    if *depth == 0 {
                        self.open.pop();
                        self.token(word, run, self.i);
                        self.continues_word = true;
                        // Like a word with a quote in it, this one is not offered for completion.
                        self.word_fragment = true;
                        return;
                    }
                }
                // A process substitution is read to its own end. Bash takes the `(` after an
                // odd run of `<` and `>` as the start of one.
                b'<' | b'>' => {
                    let mut end = self.i;
                    while matches!(self.src.get(end), Some(b'<' | b'>')) {
                        end += 1;
                    }
                    if (end - self.i) % 2 == 1 && self.src.get(end) == Some(&b'(') {
                        self.token(word, run, end - 1);
                        self.i = end - 1;
                        return self.redirect(end - 1);
                    }
                    self.i = end;
                }
                b'\\' => self.i = (self.i + 2).min(self.src.len()),
                b'\'' => {
                    self.token(word, run, self.i);
                    let quote = self.i;
                    self.i = match self.src[quote + 1..].iter().position(|&b| b == b'\'') {
                        Some(length) => quote + length + 2,
                        None => self.src.len(),
                    };
                    self.token(TokenKind::Quoted, quote, self.i);
                    run = self.i;
                }
                b'"' => {
                    self.token(word, run, self.i);
                    self.token(TokenKind::Quoted, self.i, self.i + 1);
                    self.i += 1;
                    self.open.push(Open::DoubleQuote);
                    self.double_quotes += 1;
                    return;
                }
                b'`' => {
                    self.token(word, run, self.i);
                    self.token(TokenKind::Expansion, self.i, self.i + 1);
                    self.i += 1;
                    self.backquote();
                    return;
                }
                b'$' => {
                    self.token(word, run, self.i);
                    run = self.i;
                    match self.dollar(false) {
                        Dollar::Opened => return,
                        Dollar::Consumed => run = self.i,
                        Dollar::Literal => {}
                    }
                }
                _ => self.i += 1,
            }
        }
        self.token(word, run, self.i);
    }
}

enum Dollar {
    /// A nested construct was opened; scanning continues inside it.
    Opened,
    /// A whole expansion was read.
    Consumed,
    /// The `$` stands for itself.
    Literal,
}

#[cfg(test)]
mod tests {
    use super::{
        CursorWord, Position, TokenKind, cursor_word, is_complete, is_reserved_word, scan,
    };
    use test_r::test;

    #[test]
    fn reserved_words_are_told_from_command_names() {
        for word in [
            "if", "fi", "for", "done", "case", "esac", "while", "{", "[[",
        ] {
            assert!(is_reserved_word(word), "{word}");
        }
        for word in ["find", "fixture", "file", "format", "fi x", ""] {
            assert!(!is_reserved_word(word), "{word:?}");
        }
    }

    #[test]
    fn finished_input_is_submitted() {
        for input in [
            "",
            "ls -l",
            "echo 'a b' \"c d\" $x ${y:-z} $(date) `date`",
            "echo a; echo b & wait",
            "a | b && c || d",
            "echo \\\"",
            "echo a # 'not a quote",
            "echo if then fi done esac",
            "if true; then echo a; else echo b; fi",
            "if true\nthen\n  echo a\nfi",
            "while read -r l; do echo \"$l\"; done < f",
            "for x in a b; do echo $x; done",
            "for x; do echo $x; done",
            "for ((i = 0; i < 3; i++)); do echo $i; done",
            "case $x in a|b) echo ab;; (c) echo c ;; *) ;; esac",
            "case $x in\n  a) echo a ;;\nesac",
            "f() { echo a; }",
            "function f { echo a; }",
            "{ echo a; echo b; } > f",
            "(cd /tmp && ls)",
            "[[ $a == b && -n $c ]] && echo y",
            "echo $((1 << 2)) $(( (1 + 2) * 3 ))",
            "((x++)) || true",
            "cat <<EOF\nhello\nEOF",
            "cat <<EOF\nhello\nEOF\n",
            "cat <<-EOF\n\thello\n\tEOF",
            "cat <<'E O F'\n$x\nE O F",
            "cat <<A <<B\na\nA\nb\nB",
            "cat <<<word",
            "echo \"${x:-don't}\"",
            "echo a 2>&1 >/dev/null",
            "diff <(sort a) <(sort b)",
            "echo a \\\n",
            "X=1 Y=\"a b\" cmd",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
    }

    #[test]
    fn unfinished_input_continues_on_a_new_line() {
        for input in [
            "echo 'abc",
            "echo \"abc",
            "echo \"abc\\",
            "echo $'abc",
            "echo $(date",
            "echo ${x",
            "echo `date",
            "echo $((1 + 2)",
            "(cd /tmp",
            "{ echo a;",
            "[[ $a == b",
            "echo a \\",
            "a |",
            "a |\n",
            "a &&",
            "a ||   ",
            "if true; then",
            "if true; then echo a; else",
            "while true; do",
            "for x in a b",
            "for x in a b; do echo $x",
            "case $x in",
            "case $x in a) echo a ;;",
            "f() {",
            "cat <<EOF",
            "cat <<EOF\nhello",
            "cat <<EOF\nhello\n",
            "cat <<-EOF\n\thello\n",
            "cat <<A <<B\na\nA\nb",
            "cat <<'EOF",
            "echo \"$(echo 'a",
            "if true; then\n  cat <<EOF\nx\nEOF\n",
        ] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
    }

    #[test]
    fn input_that_does_not_fit_is_sent_for_bash_to_report() {
        for input in [
            "fi",
            "done",
            "esac",
            "echo a; }",
            "echo a)",
            "echo a ;; echo b",
            "cat <<",
            "if true; then echo a; done",
        ] {
            assert!(is_complete(input), "{input:?} should be sent");
        }
    }

    fn kinds(input: &str) -> Vec<(TokenKind, &str)> {
        scan(input)
            .tokens
            .iter()
            .map(|token| (token.kind, &input[token.start..token.end]))
            .collect()
    }

    #[test]
    fn tokens_name_each_part_of_a_command() {
        use TokenKind::{Comment, Expansion, Heredoc, Operator, Quoted, Redirect, Reserved, Word};
        let command = Word { command: true };
        let plain = Word { command: false };
        assert_eq!(
            kinds("X=1 ls -l \"$HOME/a b\" | grep 'x y' > out 2>&1 # done"),
            vec![
                (plain, "X=1"),
                (command, "ls"),
                (plain, "-l"),
                (Quoted, "\""),
                (Expansion, "$HOME"),
                (Quoted, "/a b\""),
                (Operator, "|"),
                (command, "grep"),
                (Quoted, "'x y'"),
                (Redirect, ">"),
                (plain, "out"),
                (Redirect, "2>&"),
                (plain, "1"),
                (Comment, "# done"),
            ]
        );
        assert_eq!(
            kinds("if a; then b; fi"),
            vec![
                (Reserved, "if"),
                (command, "a"),
                (Operator, ";"),
                (Reserved, "then"),
                (command, "b"),
                (Operator, ";"),
                (Reserved, "fi"),
            ]
        );
        assert_eq!(
            kinds("echo $(date +%s) x"),
            vec![
                (command, "echo"),
                (Expansion, "$("),
                (command, "date"),
                (plain, "+%s"),
                (Expansion, ")"),
                (plain, "x"),
            ]
        );
        assert_eq!(
            kinds("cat <<EOF\nbody\nEOF\nls"),
            vec![
                (command, "cat"),
                (Redirect, "<<"),
                (plain, "EOF"),
                (Heredoc, "body\nEOF\n"),
                (command, "ls"),
            ]
        );
        // A word that is partly quoted is never offered as a command name.
        assert_eq!(kinds("my\"cmd\" a")[0], (plain, "my"));
        assert_eq!(
            kinds("for x in a; do f; done"),
            vec![
                (Reserved, "for"),
                (plain, "x"),
                (Reserved, "in"),
                (plain, "a"),
                (Operator, ";"),
                (Reserved, "do"),
                (command, "f"),
                (Operator, ";"),
                (Reserved, "done"),
            ]
        );
    }

    #[test]
    fn tokens_stay_on_character_boundaries() {
        let input = "echo 'h\u{e9}llo' w\u{f6}rld \"\u{65e5}\u{672c}\" # \u{2713}";
        let scan = scan(input);
        for token in &scan.tokens {
            assert!(input.is_char_boundary(token.start) && input.is_char_boundary(token.end));
            assert!(token.start < token.end && token.end <= input.len());
        }
        assert!(scan.complete);
    }

    #[test]
    fn any_keystrokes_scan_without_panicking() {
        // Every string over the characters the lexer treats specially, up to a length, by a
        // fixed pseudo-random walk: the prompt runs this on each key press.
        let alphabet: Vec<&str> = vec![
            "a", " ", "\n", "\t", "'", "\"", "`", "$", "(", ")", "{", "}", "[[", "]]", "<", ">",
            "|", "&", ";", "\\", "#", "=", "if", "then", "fi", "for", "in", "do", "done", "case",
            "esac", "<<", "EOF", "\u{e9}", "$(", "${", "$((", "((", "))", "\r", "\x0b", "\x0c",
            "\0", "\u{1b}", "\u{a0}",
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..20_000 {
            let mut input = String::new();
            for _ in 0..12 {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                input.push_str(alphabet[(state >> 33) as usize % alphabet.len()]);
            }
            let scan = scan(&input);
            let mut end = 0;
            for token in &scan.tokens {
                assert!(token.start >= end && token.start < token.end, "{input:?}");
                assert!(input.is_char_boundary(token.start), "{input:?}");
                assert!(input.is_char_boundary(token.end), "{input:?}");
                end = token.end;
            }
            assert!(end <= input.len(), "{input:?}");
            for cursor in 0..=input.len() {
                let _ = cursor_word(&input, cursor);
            }
        }
    }

    #[test]
    fn control_characters_do_not_stall_the_scan() {
        use TokenKind::Word;
        // A form feed and a carriage return separate words, as other whitespace does.
        for input in [
            "\x0c",
            "\r",
            "\r\n",
            "echo a\x0cb",
            "echo a\rb",
            "\0",
            "a\x0bb",
        ] {
            assert!(is_complete(input), "{input:?}");
        }
        assert_eq!(
            kinds("echo a\x0cb\rc"),
            vec![
                (Word { command: true }, "echo"),
                (Word { command: false }, "a"),
                (Word { command: false }, "b"),
                (Word { command: false }, "c"),
            ]
        );
    }

    #[test]
    fn a_substitution_as_the_subject_keeps_the_case_open() {
        for input in [
            "case $(uname) in",
            "case \"$(uname -s)\" in\n  Linux) echo l ;;",
            "case `uname` in\n  Linux) echo l ;;",
            "for f in $(ls); do",
        ] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
        for input in [
            "case $(uname) in Linux) echo l;; esac",
            "case \"$(uname -s)\" in\n  Linux) echo l ;;\nesac",
            "case `uname` in Linux) echo l;; esac",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        assert!(kinds("case $(uname) in a) ;; esac").contains(&(TokenKind::Reserved, "in")));
    }

    #[test]
    fn a_closed_backquote_is_finished_whatever_it_holds() {
        // Bash does not parse the text between backquotes until it runs it.
        for input in [
            "`(`",
            "`|`",
            "`#`",
            "`$(`",
            "`${`",
            "`$((`",
            "`((`",
            "`<<`",
            "`'`",
            "`\"`",
            "`if `",
            "`a |`",
            "`a[`",
            "echo `{` b",
            "echo \"`\"`\"",
            "echo \"`(`\"",
            "echo ${x:-`(`}",
            "echo `a \\` b`",
            "if true; then echo `(`; fi",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        // What follows the closing backquote is the rest of the line, not part of what the
        // text inside left open.
        use TokenKind::{Expansion, Operator, Word};
        assert_eq!(
            kinds("echo `(` b"),
            vec![
                (Word { command: true }, "echo"),
                (Expansion, "`"),
                (Operator, "("),
                (Expansion, "`"),
                (Word { command: false }, "b"),
            ]
        );
        for input in ["`(`|", "echo `(` &&", "`'`'", "if `(`; then"] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
    }

    #[test]
    fn an_unclosed_backquote_continues_whatever_it_holds() {
        for input in [
            "`",
            "`(",
            "`)",
            "`fi",
            "echo `date",
            "`a\\`",
            "`(``)",
            "`(`a`)",
            "echo \"`date",
        ] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
        // An error before the backquote is one bash reports at once.
        assert!(is_complete(") `"));
    }

    #[test]
    fn an_array_subscript_is_one_word_up_to_its_closing_bracket() {
        // At the start of a command bash reads `name[` up to the matching `]` without looking
        // for operators, blanks or reserved words in between.
        for input in [
            "a[(]",
            "a[[(]]",
            "if[[<<]]",
            "a[ ( ]=1 b",
            "a[ ; ]",
            "a[\n]",
            "a[#]",
            "a; b[(]",
            "if a[(]; then b; fi",
            "a[$(echo ])]",
            "a[<(])]",
            "a[\"]\"]",
            "a[']']",
            "a[`]`]",
            "a[\\]]",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        for input in [
            "a[",
            "a[[",
            "a[(",
            "a[[(]",
            "a['",
            "a[\"]",
            "a[$(]",
            "a[<(]",
            "{ a[; }",
            "if a[; then b; fi",
        ] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
        // Anywhere else, and after anything but a name, `[` is an ordinary character.
        for input in [
            "echo a[",
            "ls a[b",
            "echo x=1 a[",
            "./a[",
            "1a[",
            "a.b[",
            "a[1][",
            "\"a\"[",
            "case x in a[) ;; esac",
            "for x in a[; do b; done",
            "a[x][y]=1 b[",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        // So it is for the lexer once an assignment, a redirection or `time` has come before
        // the command name, where bash reads a subscript in some cases and not in others.
        for input in [
            "x=1 >f a[",
            "x=1 >f y=2 a[",
            "x=1 >$(f) a[",
            "x=1 <<E a[\nE",
            "x=1 then a[",
            "<(x) a[",
            "a | time b[",
        ] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        // And in parentheses, which may hold the words of an array rather than commands.
        for input in ["arr=(a[ b)", "arr=(\n  c\n  a[ b\n)"] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        // The next command starts afresh, and the word after a redirection is still where the
        // command name goes.
        assert!(!is_complete("x=1 >f; a["));
        assert!(!is_complete("x=1 >f\na["));
        assert_eq!(word(">f "), Some((3, String::new(), Position::Command)));
        assert_eq!(word("x=1 "), Some((4, String::new(), Position::Command)));
        // In a subscript bash takes the `(` after an odd run of `<` and `>` as the start of a
        // process substitution, which it reads to its own end.
        for input in ["a[<<(]", "a[><(]", "a[<([)]", "a[<<<(])]"] {
            assert!(is_complete(input), "{input:?} should be finished");
        }
        for input in ["a[<<<(]", "a[\\<<(]"] {
            assert!(!is_complete(input), "{input:?} should continue");
        }
        // No completion inside a subscript, or for the word that has one.
        assert_eq!(word("a[ b"), None);
        assert_eq!(word("a[ b ]"), None);
        assert_eq!(word("a[ $(c) b ]"), None);
        assert_eq!(word("a[ b ]=c"), None);
    }

    #[test]
    fn a_single_quote_in_a_parameter_expansion_is_plain_only_inside_double_quotes() {
        assert!(is_complete("echo \"${x:-'}\""));
        assert!(!is_complete("echo ${x:-'}"));
        assert!(is_complete("echo ${x:-'}'}"));
        // The double quotes that ended before the expansion no longer count.
        assert!(!is_complete("echo \"a\" ${x:-'}"));
        assert!(is_complete("echo \"a\" ${x:-'}'}"));
    }

    #[test]
    fn deeply_nested_parameter_expansions_scan_in_linear_time() {
        // The prompt scans the line again on every key press, so a long pasted line must not
        // cost the square of its nesting depth. Linear time is a few milliseconds here.
        let input = "${".repeat(50_000);
        let started = std::time::Instant::now();
        assert!(!is_complete(&input));
        let elapsed = started.elapsed();
        assert!(elapsed.as_millis() < 1_000, "took {elapsed:?}");
    }

    fn word(input: &str) -> Option<(usize, String, Position)> {
        cursor_word(input, input.len()).map(
            |CursorWord {
                 start,
                 text,
                 position,
             }| (start, text, position),
        )
    }

    #[test]
    fn the_word_under_the_cursor_knows_its_position() {
        use Position::{Argument, Command};
        assert_eq!(word(""), Some((0, String::new(), Command)));
        assert_eq!(word("gr"), Some((0, "gr".to_string(), Command)));
        assert_eq!(word("ls "), Some((3, String::new(), Argument)));
        assert_eq!(word("ls /tmp/a"), Some((3, "/tmp/a".to_string(), Argument)));
        assert_eq!(word("ls | gr"), Some((5, "gr".to_string(), Command)));
        assert_eq!(word("ls |"), Some((4, String::new(), Command)));
        assert_eq!(word("a && "), Some((5, String::new(), Command)));
        assert_eq!(word("X=1 gr"), Some((4, "gr".to_string(), Command)));
        assert_eq!(word("cat < fi"), Some((6, "fi".to_string(), Argument)));
        assert_eq!(word("echo >"), Some((6, String::new(), Argument)));
        assert_eq!(word("if tr"), Some((3, "tr".to_string(), Command)));
        assert_eq!(word("echo $(da"), Some((7, "da".to_string(), Command)));
        assert_eq!(word("./scr"), Some((0, "./scr".to_string(), Command)));
        // A reserved word may be the start of a longer command name: `fi` of `find`.
        assert_eq!(word("fi"), Some((0, "fi".to_string(), Command)));
        assert_eq!(word("ls; do"), Some((4, "do".to_string(), Command)));
        assert_eq!(word("if"), Some((0, "if".to_string(), Command)));
        // The middle of a line: only the text before the cursor counts.
        assert_eq!(
            cursor_word("ls /tm rest", 6).map(|word| word.text),
            Some("/tm".to_string())
        );
    }

    #[test]
    fn no_word_is_offered_where_completion_would_guess() {
        for input in [
            "echo 'ab",
            "echo \"ab",
            "echo ${ab",
            "echo \"x\"ab",
            "echo $HOME/ab",
            "ls *.t",
            "cd ~/a",
            "echo a # co",
            "cat <<EOF\nab",
        ] {
            assert_eq!(word(input), None, "{input:?}");
        }
        assert_eq!(cursor_word("abc", 7), None);
    }

    #[test]
    fn a_word_is_read_through_the_escapes_a_completion_writes() {
        use Position::Argument;
        let read = |input: &str, text: &str| {
            assert_eq!(
                word(input),
                Some((4, text.to_string(), Argument)),
                "{input:?}"
            );
        };
        // A backslash before a character that would otherwise mean something to bash.
        read("cat my\\ dir/", "my dir/");
        read("cat my\\ dir/fi", "my dir/fi");
        read("cat a\\*b\\(c\\)\\&", "a*b(c)&");
        read("cat a\\\\b", "a\\b");
        read("cat \\~x", "~x");
        // `$'...'` around a character that would act on the terminal.
        read("cat a$'\\e'b/", "a\u{1b}b/");
        read("cat $'\\n'$'\\t'x", "\n\tx");
        read(
            "cat $'\\x07'$'\\u200e'$'\\U0001f600'/x",
            "\u{7}\u{200e}\u{1f600}/x",
        );
        read("cat $'a b'x", "a bx");
        // An escape that is not finished, a glob, and quoting that a completion does not write.
        for input in [
            "cat my\\",
            "cat a\\ b*",
            "cat $'\\q'x",
            "cat $'\\x7'x",
            "cat 'a b'/c",
            "cat \"a b\"/c",
            "cat a$'\\e'\"b\"c",
        ] {
            assert_eq!(word(input), None, "{input:?}");
        }
    }
}

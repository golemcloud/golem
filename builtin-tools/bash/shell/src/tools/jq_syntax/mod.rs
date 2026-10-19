//! jq 1.8.2's parser, run for its diagnostics when jaq cannot load a program.
//!
//! jaq's grammar and messages are its own; jq reports a program it cannot parse with bison's
//! verbose syntax errors (`syntax error, unexpected INVALID_CHARACTER, expecting end of file`)
//! at the location of the token it could not take, recovers where its grammar says, and adds the
//! errors its grammar actions raise meanwhile (an unterminated `if`, a constant object key that is
//! not a string, an invalid string escape). This is jq's lexer (`src/lexer.l`, ported) driving
//! jq's own LALR tables (`tables.rs`, generated from jq's `src/parser.c`) with bison 3.8.2's
//! `yyparse` (error reporting and recovery included), so each of those reports is jq's, in
//! jq's order.
mod tables;

use tables::*;

/// A location, as jq's `location`: byte offsets of the first byte and one past the last.
#[derive(Clone, Copy, Default)]
struct Loc {
    start: usize,
    end: usize,
}

/// What a constant query folds to: its kind as jq names it and its value as jq shows it in a
/// message (`jv_dump_string_trunc`).
pub(crate) type Folded = (&'static str, String);

/// bison's `YYMAXDEPTH`: a parse deeper than this fails with "memory exhausted".
const YYMAXDEPTH: usize = 10_000;
/// bison's symbol number for its `error` token.
const YYSYMBOL_ERROR: i32 = 1;

/// What jq's parser makes of a program.
pub(crate) struct Parsed {
    /// jq's reports, each a complete `jq: error: ...` report ending in a newline, in the order
    /// jq gives them; none if jq's grammar takes the program.
    pub(crate) errors: Vec<String>,
    /// Whether the program has a main query, not only definitions (or nothing at all), which jq
    /// refuses (`Top-level program not given`).
    pub(crate) main: bool,
    /// How deeply the program nests: each parenthesised or bracketed query, object, `if`,
    /// `try`, `reduce`/`foreach` body, `label`, definition body, function argument, string
    /// interpolation, unary minus, pattern and `?//` alternative is one level deeper than what
    /// holds it. A flat chain (of `|` or `,` stages, `as` bindings, operands of one binary
    /// operator, path parts, string parts, object entries, pattern elements) does not nest:
    /// jaq parses, compiles and runs it without recursing once per element.
    pub(crate) depth: u32,
}

/// Parse `code` as jq does.
///
/// `fold` gives what a query's text folds to when jq takes it for a constant (`block_is_const`),
/// for the checks jq makes of object keys, module metadata and import paths as it parses.
pub(crate) fn check(code: &str, fold: &mut dyn FnMut(&str) -> Option<Folded>) -> Parsed {
    let mut parser = Parser {
        code,
        lexer: Lexer::new(code.as_bytes()),
        errors: Vec::new(),
        main: false,
        depth: 0,
        fold,
    };
    parser.parse();
    Parsed {
        errors: parser.errors,
        main: parser.main,
        depth: parser.depth,
    }
}

/// Start conditions of jq's lexer. `QqString` and `Comment` are exclusive: only their own rules
/// apply in them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    Initial,
    Paren,
    Bracket,
    Brace,
    QqInterp,
    QqString,
    Comment,
}

/// jq's lexer: flex's longest match (the earlier rule on a tie), start conditions on a stack.
struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    start: Start,
    stack: Vec<Start>,
    /// `yylloc`: the text the last rule matched, as `YY_USER_ACTION` sets it for every rule, so
    /// at the end of input it is still the last text matched (a token, blanks or a comment).
    loc: Loc,
}

/// jq's fixed-text rules, in their order in `lexer.l`.
const FIXED: [(&[u8], i32); 33] = [
    (b"!=", NEQ),
    (b"==", EQ),
    (b"as", AS),
    (b"import", IMPORT),
    (b"include", INCLUDE),
    (b"module", MODULE),
    (b"def", DEF),
    (b"if", IF),
    (b"then", THEN),
    (b"else", ELSE),
    (b"elif", ELSE_IF),
    (b"and", AND),
    (b"or", OR),
    (b"end", END),
    (b"reduce", REDUCE),
    (b"foreach", FOREACH),
    (b"//", DEFINEDOR),
    (b"try", TRY),
    (b"catch", CATCH),
    (b"label", LABEL),
    (b"break", BREAK),
    (b"$__loc__", LOC),
    (b"|=", SETPIPE),
    (b"+=", SETPLUS),
    (b"-=", SETMINUS),
    (b"*=", SETMULT),
    (b"/=", SETDIV),
    (b"%=", SETMOD),
    (b"//=", SETDEFINEDOR),
    (b"<=", LESSEQ),
    (b">=", GREATEREQ),
    (b"..", REC),
    (b"?//", ALTERNATION),
];

/// What a rule of the general start conditions does with its match.
#[derive(Clone, Copy)]
enum Rule {
    Comment,
    Token(i32),
    Open(u8),
    Close(u8),
    Format,
    Literal,
    QqStart,
    Ident,
    Field,
    Binding,
    Blank,
    Invalid,
}

const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

const fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The length of `[a-zA-Z_][a-zA-Z_0-9]*` at the start of `text`.
fn word_len(text: &[u8]) -> usize {
    match text.first() {
        Some(&byte) if is_ident_start(byte) => {
            1 + text[1..]
                .iter()
                .take_while(|byte| is_ident_byte(**byte))
                .count()
        }
        _ => 0,
    }
}

/// The length of `([a-zA-Z_][a-zA-Z_0-9]*::)*[a-zA-Z_][a-zA-Z_0-9]*` at the start of `text`.
fn ident_len(text: &[u8]) -> usize {
    let mut len = word_len(text);
    if len == 0 {
        return 0;
    }
    while text[len..].starts_with(b"::") {
        let next = word_len(&text[len + 2..]);
        if next == 0 {
            break;
        }
        len += 2 + next;
    }
    len
}

/// The length of `([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?` at the start of `text`.
fn number_len(text: &[u8]) -> usize {
    let digits = |from: usize| {
        text[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut len = digits(0);
    if len > 0 {
        if text.get(len) == Some(&b'.') {
            len += 1 + digits(len + 1);
        }
    } else if text.first() == Some(&b'.') && digits(1) > 0 {
        len = 1 + digits(1);
    } else {
        return 0;
    }
    if matches!(text.get(len), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(text.get(len + 1), Some(b'+' | b'-')));
        let exponent = digits(len + 1 + sign);
        if exponent > 0 {
            len += 1 + sign + exponent;
        }
    }
    len
}

impl<'a> Lexer<'a> {
    const fn new(src: &'a [u8]) -> Self {
        Self {
            src,
            pos: 0,
            start: Start::Initial,
            stack: Vec::new(),
            loc: Loc { start: 0, end: 0 },
        }
    }

    fn push(&mut self, start: Start) {
        self.stack.push(self.start);
        self.start = start;
    }

    fn pop(&mut self) {
        self.start = self.stack.pop().unwrap_or(Start::Initial);
    }

    /// Consume `len` bytes as the text a rule matched (`YY_USER_ACTION`).
    fn matched(&mut self, len: usize) -> &'a [u8] {
        let text = &self.src[self.pos..self.pos + len];
        self.loc = Loc {
            start: self.pos,
            end: self.pos + len,
        };
        self.pos += len;
        text
    }

    /// `yylex`: the next token's code (0 at the end of input), and the message jq reports for
    /// an invalid literal or string escape it just read.
    fn next(&mut self) -> (i32, Option<String>) {
        loop {
            let rest = &self.src[self.pos..];
            let Some(&first) = rest.first() else {
                if self.start == Start::Comment {
                    self.pop();
                    continue;
                }
                return (0, None);
            };
            match self.start {
                Start::Comment => {
                    let (len, ends) = if rest.starts_with(b"\\\\") || rest.starts_with(b"\\\n") {
                        (2, false)
                    } else if rest.starts_with(b"\\\r\n") {
                        (3, false)
                    } else if rest.starts_with(b"\r\n") {
                        (2, true)
                    } else {
                        (1, first == b'\n')
                    };
                    self.matched(len);
                    if ends {
                        self.pop();
                    }
                }
                Start::QqString => return self.next_in_string(rest, first),
                _ => {
                    if let Some(token) = self.next_general(rest, first) {
                        return token;
                    }
                }
            }
        }
    }

    /// The rules of `IN_QQSTRING`.
    fn next_in_string(&mut self, rest: &[u8], first: u8) -> (i32, Option<String>) {
        if rest.starts_with(b"\\(") {
            self.matched(2);
            self.push(Start::QqInterp);
            return (QQSTRING_INTERP_START, None);
        }
        if first == b'"' {
            self.matched(1);
            self.pop();
            return (QQSTRING_END, None);
        }
        if first != b'\\' {
            let len = rest
                .iter()
                .take_while(|b| !matches!(b, b'\\' | b'"'))
                .count();
            self.matched(len);
            return (QQSTRING_TEXT, None);
        }
        // `(\\[^u(]|\\u[a-zA-Z0-9]{0,4})+`, passed to jq's JSON parser.
        let mut len = 0;
        while rest.get(len) == Some(&b'\\') {
            match rest.get(len + 1) {
                None | Some(b'(') => break,
                Some(b'u') => {
                    len += 2;
                    len += rest[len..]
                        .iter()
                        .take(4)
                        .take_while(|b| b.is_ascii_alphanumeric())
                        .count();
                }
                Some(_) => len += 2,
            }
        }
        if len == 0 {
            // A backslash ending the input.
            self.matched(1);
            return (INVALID_CHARACTER, None);
        }
        let text = self.matched(len);
        let mut quoted = Vec::with_capacity(len + 2);
        quoted.push(b'"');
        quoted.extend_from_slice(text);
        quoted.push(b'"');
        (
            QQSTRING_TEXT,
            jaq_json::jv_parse::parse_sized(&quoted).err(),
        )
    }

    /// The rules of the other start conditions: the longest match, the earlier rule on a tie.
    /// `None` for text that makes no token (blanks, a comment's start).
    fn next_general(&mut self, rest: &[u8], first: u8) -> Option<(i32, Option<String>)> {
        let mut best = (0, Rule::Invalid);
        let mut consider = |len: usize, rule: Rule| {
            if len > best.0 {
                best = (len, rule);
            }
        };
        if first == b'#' {
            consider(1, Rule::Comment);
        }
        for (text, token) in FIXED {
            if rest.starts_with(text) {
                consider(text.len(), Rule::Token(token));
            }
        }
        match first {
            b'.' | b'?' | b'=' | b';' | b',' | b':' | b'|' | b'+' | b'-' | b'*' | b'/' | b'%'
            | b'$' | b'<' | b'>' => consider(1, Rule::Token(i32::from(first))),
            b'[' | b'{' | b'(' => consider(1, Rule::Open(first)),
            b']' | b'}' | b')' => consider(1, Rule::Close(first)),
            b'@' => {
                let len = rest[1..].iter().take_while(|b| is_ident_byte(**b)).count();
                if len > 0 {
                    consider(1 + len, Rule::Format);
                }
            }
            b'"' => consider(1, Rule::QqStart),
            _ => (),
        }
        consider(number_len(rest), Rule::Literal);
        consider(ident_len(rest), Rule::Ident);
        if first == b'.' {
            let len = word_len(&rest[1..]);
            if len > 0 {
                consider(1 + len, Rule::Field);
            }
        }
        if first == b'$' {
            let len = ident_len(&rest[1..]);
            if len > 0 {
                consider(1 + len, Rule::Binding);
            }
        }
        consider(
            rest.iter()
                .take_while(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
                .count(),
            Rule::Blank,
        );
        if first != b'\n' {
            consider(1, Rule::Invalid);
        }
        let (len, rule) = best;
        let text = self.matched(len);
        let token = match rule {
            Rule::Comment => {
                self.push(Start::Comment);
                return None;
            }
            Rule::Blank => return None,
            Rule::Token(token) => token,
            Rule::Open(byte) => {
                self.push(match byte {
                    b'(' => Start::Paren,
                    b'[' => Start::Bracket,
                    _ => Start::Brace,
                });
                i32::from(byte)
            }
            Rule::Close(byte) => {
                let (expected, token) = match self.start {
                    Start::Paren => (b')', i32::from(b')')),
                    Start::Bracket => (b']', i32::from(b']')),
                    Start::Brace => (b'}', i32::from(b'}')),
                    Start::QqInterp => (b')', QQSTRING_INTERP_END),
                    _ => return Some((INVALID_CHARACTER, None)),
                };
                if byte != expected {
                    return Some((INVALID_CHARACTER, None));
                }
                self.pop();
                token
            }
            Rule::Format => FORMAT,
            Rule::Literal => {
                return Some((LITERAL, jaq_json::jv_parse::parse_sized(text).err()));
            }
            Rule::QqStart => {
                self.push(Start::QqString);
                QQSTRING_START
            }
            Rule::Ident => IDENT,
            Rule::Field => FIELD,
            Rule::Binding => BINDING,
            Rule::Invalid => INVALID_CHARACTER,
        };
        Some((token, None))
    }
}

/// bison's `yytnamerr`: a symbol's name without the double quotes it needs only in `yytname`.
fn symbol_name(symbol: i32) -> String {
    let name = YYTNAME[usize::try_from(symbol).unwrap_or(2)];
    let Some(inner) = name.strip_prefix('"') else {
        return name.to_owned();
    };
    let mut stripped = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | ',' => return name.to_owned(),
            '\\' => match chars.next() {
                Some('\\') => stripped.push('\\'),
                _ => return name.to_owned(),
            },
            '"' => return stripped,
            c => stripped.push(c),
        }
    }
    name.to_owned()
}

/// `YYTRANSLATE`: the symbol for a token code.
fn translate(token: i32) -> i32 {
    usize::try_from(token)
        .ok()
        .filter(|_| token <= YYMAXUTOK)
        .map_or(2, |index| i32::from(YYTRANSLATE[index]))
}

fn table(index: i32) -> Option<(i32, i32)> {
    let index = usize::try_from(index).ok().filter(|_| index <= YYLAST)?;
    Some((i32::from(YYCHECK[index]), i32::from(YYTABLE[index])))
}

/// bison's `yysyntax_error`: the unexpected token, and the expected ones when there are at most
/// four.
fn syntax_error(state: usize, token: Option<i32>) -> String {
    let Some(token) = token else {
        return "syntax error".into();
    };
    let mut message = format!("syntax error, unexpected {}", symbol_name(token));
    let pact = i32::from(YYPACT[state]);
    if pact == YYPACT_NINF {
        return message;
    }
    let mut expected = Vec::new();
    let begin = if pact < 0 { -pact } else { 0 };
    let end = (YYLAST - pact + 1).min(YYNTOKENS);
    for symbol in begin..end {
        if let Some((check, action)) = table(symbol + pact)
            && check == symbol
            && symbol != YYSYMBOL_ERROR
            && action != YYTABLE_NINF
        {
            if expected.len() == 4 {
                return message;
            }
            expected.push(symbol);
        }
    }
    for (index, symbol) in expected.into_iter().enumerate() {
        message.push_str(if index == 0 { ", expecting " } else { " or " });
        message.push_str(&symbol_name(symbol));
    }
    message
}

struct Parser<'a, 'f> {
    code: &'a str,
    lexer: Lexer<'a>,
    errors: Vec<String>,
    main: bool,
    depth: u32,
    fold: &'f mut dyn FnMut(&str) -> Option<Folded>,
}

/// What `yyparse` does next in a state.
enum Step {
    Shift(usize),
    Reduce(usize),
    Error,
}

impl Parser<'_, '_> {
    /// jq's `yyerror`/`FAIL`: `locfile_locate`'s report of `message` at `loc`.
    fn fail(&mut self, loc: Loc, message: &str) {
        let code = self.code.as_bytes();
        let start = loc.start.min(code.len().saturating_sub(1));
        let line_start = code[..start]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        let line_end = code[line_start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(code.len(), |i| line_start + i);
        let line = code[..line_start].iter().filter(|b| **b == b'\n').count() + 1;
        let end = loc.end.min(line_end.max(start + 1)).max(start);
        let text = &code[line_start..line_end];
        // `%.*s` stops at a NUL byte.
        let text = &text[..text.iter().position(|b| *b == 0).unwrap_or(text.len())];
        self.errors.push(format!(
            "jq: error: {message} at <top-level>, line {line}, column {}:\n    {}\n    {}{}\n",
            start - line_start + 1,
            String::from_utf8_lossy(text),
            " ".repeat(start - line_start),
            "^".repeat(end - start),
        ));
    }

    fn fold(&mut self, loc: Loc) -> Option<Folded> {
        let text = self.code.get(loc.start..loc.end)?;
        (self.fold)(text)
    }

    /// The error checks of jq's grammar actions, for the rule just reduced: `rhs` holds the
    /// locations of its symbols, `whole` the rule's.
    fn action(&mut self, rule: usize, rhs: &[Loc], whole: Loc) {
        const FIELD_NAME: &str = "try .[\"field\"] instead of .field for unusually named fields";
        const KEY_PARENTHESES: &str = "May need parentheses around object key expression";
        match rule {
            RULE_MODULE_META | RULE_IMPORT_META => match self.fold(rhs[1]) {
                None => self.fail(rhs[1], "Module metadata must be constant"),
                Some((kind, _)) if kind != "object" => {
                    self.fail(rhs[1], "Module metadata must be an object");
                }
                Some(_) => (),
            },
            RULE_IMPORT_FROM => {
                if self.fold(rhs[0]).is_none() {
                    self.fail(rhs[0], "Import path must be constant");
                }
            }
            RULE_BREAK_ERROR => self.fail(whole, "break requires a label to break to"),
            RULE_DOT_ERROR | RULE_DOT_IDENT_ERROR => self.fail(whole, FIELD_NAME),
            RULE_IF_ERROR => self.fail(whole, "Possibly unterminated 'if' statement"),
            RULE_TRY_ERROR => self.fail(whole, "Possibly unterminated 'try' statement"),
            RULE_OBJ_PAT_KEY | RULE_DICT_PAIR_KEY => {
                if let Some((kind, dump)) = self.fold(rhs[1])
                    && kind != "string"
                {
                    self.fail(rhs[1], &format!("Cannot use {kind} ({dump}) as object key"));
                }
            }
            RULE_OBJ_PAT_ERROR => self.fail(whole, KEY_PARENTHESES),
            RULE_DICT_PAIR_ERROR => self.fail(rhs[0], KEY_PARENTHESES),
            RULE_TOP_LEVEL_QUERY => self.main = true,
            _ => (),
        }
    }

    /// Read the lookahead token if there is none (`yychar == YYEMPTY`); jq's `yylex` reports an
    /// invalid literal as it reads it.
    fn lookahead(&mut self, lookahead: &mut Option<i32>) -> i32 {
        if let Some(token) = *lookahead {
            return token;
        }
        let (token, error) = self.lexer.next();
        if let Some(error) = error {
            self.fail(self.lexer.loc, &error);
        }
        let token = token.max(0);
        *lookahead = Some(token);
        token
    }

    /// bison 3.8.2's `yyparse`, with locations and without semantic values.
    fn parse(&mut self) {
        let mut states: Vec<usize> = vec![0];
        let mut locs: Vec<Loc> = vec![self.lexer.loc];
        // How deeply each symbol on the stack nests (see `Parsed::depth`).
        let mut depths: Vec<u32> = vec![0];
        let mut lookahead: Option<i32> = None;
        let mut errstatus = 0;
        loop {
            let state = states.last().copied().unwrap_or(0);
            if states.len() >= YYMAXDEPTH {
                self.fail(self.lexer.loc, "memory exhausted");
                return;
            }
            if state == YYFINAL {
                return;
            }
            let pact = i32::from(YYPACT[state]);
            let mut step = None;
            if pact != YYPACT_NINF {
                let token = translate(self.lookahead(&mut lookahead));
                if let Some((check, action)) = table(pact + token)
                    && check == token
                {
                    step = Some(match action {
                        YYTABLE_NINF => Step::Error,
                        action if action <= 0 => Step::Reduce((-action).unsigned_abs() as usize),
                        action => Step::Shift(action.unsigned_abs() as usize),
                    });
                }
            }
            let step = step.unwrap_or_else(|| match YYDEFACT[state] {
                0 => Step::Error,
                rule => Step::Reduce(usize::from(rule)),
            });
            match step {
                Step::Shift(next) => {
                    if errstatus > 0 {
                        errstatus -= 1;
                    }
                    states.push(next);
                    locs.push(self.lexer.loc);
                    depths.push(0);
                    lookahead = None;
                }
                Step::Reduce(rule) => {
                    let len = usize::from(YYR2[rule]);
                    let top = locs.len();
                    let whole = if len > 0 {
                        Loc {
                            start: locs[top - len].start,
                            end: locs[top - 1].end,
                        }
                    } else {
                        let below = locs[top - 1].end;
                        Loc {
                            start: below,
                            end: below,
                        }
                    };
                    let rhs = locs[top - len..].to_vec();
                    self.action(rule, &rhs, whole);
                    let depth = depths[top - len..]
                        .iter()
                        .enumerate()
                        .map(|(position, depth)| {
                            let bit = 1 << position;
                            depth + u32::from(YYNEST[rule] & bit != 0)
                        })
                        .max()
                        .unwrap_or(0);
                    self.depth = self.depth.max(depth);
                    states.truncate(states.len() - len);
                    locs.truncate(top - len);
                    depths.truncate(top - len);
                    locs.push(whole);
                    depths.push(depth);
                    let below = states.last().copied().unwrap_or(0);
                    let lhs = usize::from(YYR1[rule]) - YYNTOKENS.unsigned_abs() as usize;
                    let goto = i32::from(YYPGOTO[lhs]) + i32::try_from(below).unwrap_or(0);
                    let next = match table(goto) {
                        Some((check, action)) if usize::try_from(check) == Ok(below) => {
                            action.unsigned_abs() as usize
                        }
                        _ => usize::from(YYDEFGOTO[lhs]),
                    };
                    states.push(next);
                }
                Step::Error => {
                    // yyerrlab
                    if errstatus == 0 {
                        let message = syntax_error(state, lookahead.map(translate));
                        self.fail(self.lexer.loc, &message);
                    }
                    let mut error_start = self.lexer.loc;
                    if errstatus == 3 {
                        match lookahead {
                            Some(0) => return,
                            Some(_) => lookahead = None,
                            None => (),
                        }
                    }
                    // yyerrlab1: pop until a state shifts the error token.
                    errstatus = 3;
                    let target = loop {
                        let state = states.last().copied().unwrap_or(0);
                        let pact = i32::from(YYPACT[state]);
                        if pact != YYPACT_NINF
                            && let Some((check, action)) = table(pact + YYSYMBOL_ERROR)
                            && check == YYSYMBOL_ERROR
                            && action > 0
                        {
                            break action.unsigned_abs() as usize;
                        }
                        if states.len() == 1 {
                            return;
                        }
                        error_start = locs.last().copied().unwrap_or_default();
                        states.pop();
                        locs.pop();
                        depths.pop();
                    };
                    locs.push(Loc {
                        start: error_start.start,
                        end: self.lexer.loc.end,
                    });
                    depths.push(0);
                    states.push(target);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Folded, Parsed, check};

    /// jq's fold of a few constants, for the parse-time checks.
    fn fold(text: &str) -> Option<Folded> {
        match text {
            "1" => Some(("number", "1".into())),
            "\"a\"" => Some(("string", "\"a\"".into())),
            _ => None,
        }
    }

    fn parse(code: &str) -> Parsed {
        check(code, &mut fold)
    }

    #[test]
    fn takes_what_jq_takes() {
        for code in [
            ".",
            ".a[1:] | {a, b: .c, (\"a\"): 2}",
            "def f(x): x; f(.) // 1",
            "$__loc__",
        ] {
            let parsed = parse(code);
            assert!(parsed.errors.is_empty(), "{code}: {:?}", parsed.errors);
            assert!(parsed.main, "{code}");
        }
    }

    #[test]
    fn a_program_of_definitions_has_no_main_query() {
        for code in ["", "  # just a comment", "def f: 1;"] {
            let parsed = parse(code);
            assert!(parsed.errors.is_empty() && !parsed.main, "{code}");
        }
    }

    #[test]
    fn syntax_errors_are_bisons() {
        assert_eq!(
            parse(".[").errors,
            [
                "jq: error: syntax error, unexpected end of file at <top-level>, line 1, column 2:\n    .[\n     ^\n"
            ]
        );
        assert_eq!(
            parse(".[1:2:3]").errors,
            [
                "jq: error: syntax error, unexpected ':', expecting '|' or ',' or ']' at <top-level>, line 1, column 6:\n    .[1:2:3]\n         ^\n"
            ]
        );
        assert_eq!(
            parse(")").errors,
            [
                "jq: error: syntax error, unexpected INVALID_CHARACTER, expecting end of file at <top-level>, line 1, column 1:\n    )\n    ^\n"
            ]
        );
    }

    #[test]
    fn recovery_reports_what_jqs_grammar_actions_do() {
        assert_eq!(
            parse("if . then 1").errors,
            [
                "jq: error: syntax error, unexpected end of file at <top-level>, line 1, column 11:\n    if . then 1\n              ^\n",
                "jq: error: Possibly unterminated 'if' statement at <top-level>, line 1, column 1:\n    if . then 1\n    ^^^^^^^^^^^\n",
            ]
        );
    }

    #[test]
    fn a_constant_key_is_checked_as_it_is_parsed() {
        assert_eq!(
            parse("{(1):2} | )").errors,
            [
                "jq: error: Cannot use number (1) as object key at <top-level>, line 1, column 3:\n    {(1):2} | )\n      ^\n",
                "jq: error: syntax error, unexpected INVALID_CHARACTER at <top-level>, line 1, column 11:\n    {(1):2} | )\n              ^\n",
            ]
        );
        assert!(parse("{(\"a\"): 1}").errors.is_empty());
    }

    #[test]
    fn an_invalid_byte_is_located_by_byte() {
        let errors = parse("1 + \u{e9}").errors;
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("unexpected INVALID_CHARACTER"),
            "{errors:?}"
        );
        assert!(errors[0].contains("column 5:"), "{errors:?}");
    }
}

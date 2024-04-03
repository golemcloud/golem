use std::fmt::Display;
use std::str::Chars;

use regex::Regex;

use super::cursor::TokenCursor;

// Typical usage:
//
// let result = Tokenizer::new("input_value").run();
// let cursor = result.to_cursor();
//   while let Some(token) = cursor.next_token() {}
// }
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    MultiChar(MultiCharTokens),
    RCurly,
    LCurly,
    LSquare,
    RSquare,
    GreaterThan,
    Space,
    NewLine,
    LParen,
    RParen,
    Dot,
    Comma,
    Quote,
}

#[derive(Clone, PartialEq, Debug)]
enum MultiCharTokens {
    Worker,
    Request,
    Ok,
    Err,
    Some,
    None,
    Match,
    InterpolationStart,
    GreaterThanOrEqualTo,
    LessThanOrEqualTo,
    EqualTo,
    If,
    Then,
    Else,
    Arrow,
    Other(String)
}

impl Display for MultiCharTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                MultiCharTokens::Else => "else",
                MultiCharTokens::EqualTo => "==",
                MultiCharTokens::InterpolationStart => "${",
                MultiCharTokens::GreaterThanOrEqualTo => ">=",
                MultiCharTokens::LessThanOrEqualTo => "<=",
                MultiCharTokens::If => "if",
                MultiCharTokens::Then => "then",
                MultiCharTokens::Worker => "worker",
                MultiCharTokens::Request => "request",
                MultiCharTokens::Ok => "ok",
                MultiCharTokens::Err => "err",
                MultiCharTokens::Some => "some",
                MultiCharTokens::None => "none",
                MultiCharTokens::Match => "match",
                MultiCharTokens::Arrow => "=>",
                MultiCharTokens::Other(string) => string.as_str()
            })
        }
}


impl Token {
    // If a token needs to be considered as only a raw string
    pub fn as_raw_string_token(&self) -> Token {
        match self {
            Token::MultiChar(MultiCharTokens::InterpolationStart) => self.clone(), /* We disallow any normalisation to string if the token is interpolation! */
            Token::RCurly => self.clone(),
            token => Token::MultiChar(MultiCharTokens::Other(token.to_string())),
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Token::Space => " ",
                Token::RCurly => "}",
                Token::GreaterThan => ">",
                Token::LParen => "(",
                Token::RParen => ")",
                Token::NewLine => "\n",
                Token::LSquare => "[",
                Token::RSquare => "]",
                Token::Dot => ".",
                Token::LCurly => "{",
                Token::Comma => ",",
                Token::Quote => "'",
                Token::MultiChar(multi_char) => multi_char.to_string().as_str()
            }
        )
    }
}

impl Token {
    pub fn is_non_empty_constructor(&self) -> bool {
        matches!(self, Token::MultiChar(MultiCharTokens::Ok) | Token::MultiChar(MultiCharTokens::Err) | Token::MultiChar(MultiCharTokens::Some) | Token::MultiChar(MultiCharTokens::Match))
    }

    pub fn is_empty_constructor(&self) -> bool {
        matches!(self, Token::MultiChar(MultiCharTokens::None))
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::RawString(string) => string.is_empty(),
            _ => false,
        }
    }
}

// Vec<Token>
// Vec<PlaceHolder> Space Vec<PlaceHolder>

struct State {
    pos: usize,
    state: TokenizerState,
}
#[derive(Clone)]
enum TokenizerState {
    Beginning,
    Text,
    Static(Token),
    End,
}

impl TokenizerState {
    fn is_end(&self) -> bool {
        match self {
            Self::Beginning => false,
            Self::Text => false,
            Self::End => true,
            Self::Static(_) => false,
        }
    }
}

pub struct Tokenizer<'a> {
    text: &'a str,
    state: State,
}

impl<'t> Tokenizer {

    fn next_chars(&self) -> Chars<'t> {
        self.text.get(self.state.pos..).unwrap().chars()
    }

    pub fn eat_while(&mut self, f: impl Fn(char) -> bool) -> Option<&str> {
        let beginning = self.state.pos;

        self.state.pos += self
            .next_chars()
            .map_while(|ch| f(ch).then(|| ch.len_utf8()))
            .sum::<usize>();

        self.text.get(beginning..self.state.pos)
    }

    pub fn rest(&self) -> &str {
        &self.text[self.state.pos..]
    }

    pub fn progress_by(&mut self, ch: &char) {
        self.state.pos += ch.len_utf8();
    }

    pub fn new(text: &'t str) -> Self {
        Self {
            text,
            state: State {
                pos: 0,
                state: TokenizerState::Beginning,
            }
        }
    }

    // Collect tokens gets rid of empty spaces and make sure everything is trimmed
    // If needed raw versions as it is, use `.collect()` instead of `.collect_tokens`
    pub fn run(self) -> TokeniserResult {
        let all_tokens: Vec<Token> = self.collect();

        TokeniserResult {
            value: all_tokens
                .into_iter()
                .flat_map(|x: Token| if x.is_empty() { vec![] } else { vec![x] })
                .collect(),
        }
    }

    fn get_token(&mut self) -> Token {
        let mut token: Option<Token> = None;

        for (character_index, c) in tokenise_string_with_index(self.text.as_str()) {
            if c == "<" {
                token = Some(Token::RawString(self.text[..character_index].to_string())); // Example: we token out the string just before <, and marks the state as <
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::LessThan);
                break;
            } else if c == "<=" {
                token = Some(Token::RawString(self.text[..character_index].to_string())); // Example: we token out the string just before <, and marks the state as <
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::LessThanOrEqualTo);
                break;
            } else if c == ">" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::GreaterThan);
                break;
            } else if c == ">=" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::GreaterThanOrEqualTo);
                break;
            } else if c == "==" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::EqualTo);
                break;
            } else if c == "if" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::If);
                break;
            } else if c == "then" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 4..].to_string();
                self.state = TokenizerState::Static(Token::Then);
                break;
            } else if c == "else" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 4..].to_string();
                self.state = TokenizerState::Static(Token::Else);
                break;
            } else if c == "(" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::LParen);
                break;
            } else if c == ")" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::RParen);
                break;
            } else if c == "[" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::LSquare);
                break;
            } else if c == "]" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::RSquare);
                break;
            } else if c == "${" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::InterpolationStart);
                break;
            } else if c == "=>" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 2..].to_string();
                self.state = TokenizerState::Static(Token::Arrow);
                break;
            } else if c == "{" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::LCurly);
                break;
            } else if c == "}" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::RCurly);
                break;
            } else if c == " " {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::Space);
                break;
            } else if c == "\n" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::NewLine);
                break;
            } else if c == "." {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + 1..].to_string();
                self.state = TokenizerState::Static(Token::Dot);
                break;
            } else if c == "worker" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Worker.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Worker);
                break;
            } else if c == "request" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Request.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Request);
                break;
            } else if c == "ok" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + Token::Ok.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Ok);
                break;
            } else if c == "err" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + Token::Err.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Err);
                break;
            } else if c == "some" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Some.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Some);
                break;
            } else if c == "none" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::None.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::None);
                break;
            } else if c == "null" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text = self.text[character_index + "null".to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::None);
                break;
            } else if c == "match" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Match.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Match);
                break;
            } else if c == "," {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Comma.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Comma);
                break;
            } else if c == "'" {
                token = Some(Token::RawString(self.text[..character_index].to_string()));
                self.text =
                    self.text[character_index + Token::Quote.to_string().len()..].to_string();
                self.state = TokenizerState::Static(Token::Quote);
                break;
            }
        }

        match token {
            Some(token) => token,
            None => {
                token = Some(Token::RawString(self.text.clone()));
                self.text = "".to_string();
                self.state = TokenizerState::End;
                token.unwrap()
            }
        }
    }

    fn get_single_char_token(&mut self) -> Option<Token> {
        let ch = self.text.chars().next()?;
        if let Some(token) =  match ch {
            ',' => Some(Token::Comma),
            '{' => Some(Token::LCurly),
            '}' => Some(Token::RCurly),
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            '[' => Some(Token::LSquare),
            ']' => Some(Token::RSquare),
            '.' => Some(Token::Dot),
            '\'' => Some(Token::Quote),
            '\n' => Some(Token::NewLine),
            ' ' => Some(Token::Space),
            '>' => Some(Token::GreaterThan),
            _ => None,
        } {
            self.progress();
            Some(token)
        }  else {
            None
        }
    }

    fn get_multi_char_token(&mut self) -> Option<Token> {
        let ch = self.text.chars().next()?;
        let token = match ch {
            'a'..='z' | 'A'..='Z' => {
                // Eat characters from kebab-names (ascii alphanumeric and dash)
                let str = self.eat_while(|ch| ch.is_ascii_alphanumeric() || ch == '-');
                match str {
                    Some("worker") => Token::MultiChar(MultiCharTokens::Worker),
                    Some("request") =>Token::MultiChar(MultiCharTokens::Request),
                    Some("ok") => Token::MultiChar(MultiCharTokens::Ok),
                    Some("err") => Token::MultiChar(MultiCharTokens::Err),
                    Some("some") => Token::MultiChar(MultiCharTokens::Some),
                    Some("none") => Token::MultiChar(MultiCharTokens::None),
                    Some("match") => Token::MultiChar(MultiCharTokens::Match),
                    Some("if") => Token::MultiChar(MultiCharTokens::If),
                    Some("then") => Token::MultiChar(MultiCharTokens::Then),
                    Some("else") => Token::MultiChar(MultiCharTokens::Else),
                    Some("${") =>  Token::MultiChar(MultiCharTokens::InterpolationStart),
                    Some(">=") => Token::MultiChar(MultiCharTokens::GreaterThanOrEqualTo),
                    Some("<=") => Token::MultiChar(MultiCharTokens::LessThanOrEqualTo),
                    Some("==") => Token::MultiChar(MultiCharTokens::EqualTo),
                    Some("=>") => Token::MultiChar(MultiCharTokens::Arrow),
                    Some(chars) => Token::MultiChar(MultiCharTokens::Other(chars.to_string())),
                
                }
            }
            '0'..='9' => {
                // Eat characters from numbers (including decimals and exponents)
                self.eat_while(|ch| matches!(ch, '0'..='9' | '-' | '.' | 'e' | 'E' | '+'));
                Token::Number
            }
            '\'' => {
                self.eat_string('\'')?;
                Token::Char
            }
            '"' => {
                self.eat_string('"')?;
                Token::String
            }
            _ => return Err(LexError::UnexpectedChar(self.pos)),
        };
    }
}

#[derive(Debug, Clone)]
pub struct TokeniserResult {
    pub value: Vec<Token>,
}

impl TokeniserResult {
    pub fn to_cursor(&self) -> TokenCursor {
        TokenCursor::new(self.value.clone())
    }
}


fn get_multi_char_tokens(ch: char) -> Option<Token> {

}

fn tokenise_string_with_index(input_string: &str) -> Vec<(usize, &str)> {
    let mut result: Vec<(usize, &str)> = Vec::new();
    let mut current_index = 0;
    let token_regex_pattern = Regex::new(
        r"(worker|request|,|\.|'|<=|\$\{|}|>=|\n| |==|<|>|\bif\b|\bthen\b|\belse\b|=>|\{|\bsome\b|\bnone\b|\bmatch\b|\bok\b|\berr\b|[ -]|[^\s])|[\(\)]|\[|\]|(\w+)",
    )
        .unwrap();

    for capture in token_regex_pattern.captures_iter(input_string) {
        if let Some(matched) = capture.get(1) {
            let matched_str = matched.as_str();
            result.push((current_index, matched_str));
            current_index += matched_str.len();
        } else if let Some(matched) = capture.get(2) {
            let matched_str = matched.as_str();
            result.push((current_index, matched_str));
            current_index += matched_str.len();
        }
    }

    result
}

impl Iterator for Tokenizer {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        if self.state.is_end() {
            return None;
        }

        match self.state.clone() {
            TokenizerState::Beginning => Some(self.get_token()),
            TokenizerState::Text => Some(self.get_token()),
            TokenizerState::Static(inner) => {
                self.state = TokenizerState::Text;
                Some(inner)
            }
            TokenizerState::End => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{Token, Tokenizer};

    extern crate alloc;

    #[test]
    fn test_raw() {
        let tokens: Vec<Token> = Tokenizer::new("foo bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::RawString("foo".to_string()),
                Token::Space,
                Token::RawString("bar".to_string())
            ]
        );
    }

    #[test]
    fn test_open_close_braces() {
        let tokens: Vec<Token> = Tokenizer::new("(foo bar)").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::LParen,
                Token::RawString("foo".to_string()),
                Token::Space,
                Token::RawString("bar".to_string()),
                Token::RParen
            ]
        );
    }

    #[test]
    fn test_dot() {
        let tokens: Vec<Token> = Tokenizer::new("foo . bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::RawString("foo".to_string()),
                Token::Space,
                Token::Dot,
                Token::Space,
                Token::RawString("bar".to_string()),
            ]
        );
    }

    #[test]
    fn test_request() {
        let tokens: Vec<Token> = Tokenizer::new("request .").run().value;
        assert_eq!(tokens, vec![Token::Request, Token::Space, Token::Dot,]);
    }

    #[test]
    fn test_worker_response() {
        let tokens: Vec<Token> = Tokenizer::new("worker.").run().value;
        assert_eq!(tokens, vec![Token::Worker, Token::Dot]);
    }

    #[test]
    fn test_open_close_square_bracket() {
        let tokens: Vec<Token> = Tokenizer::new("[foo bar]").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::LSquare,
                Token::RawString("foo".to_string()),
                Token::Space,
                Token::RawString("bar".to_string()),
                Token::RSquare
            ]
        );
    }

    #[test]
    fn test_if_start() {
        let tokens: Vec<Token> = Tokenizer::new("if x").run().value;

        assert_eq!(
            tokens,
            vec![Token::If, Token::Space, Token::RawString("x".to_string()),]
        );
    }

    #[test]
    fn test_false_ifs() {
        let tokens: Vec<Token> = Tokenizer::new("asif x").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("asif".to_string()),
                Token::Space,
                Token::RawString("x".to_string())
            ]
        );
    }

    #[test]
    fn test_false_ifs2() {
        let tokens: Vec<Token> = Tokenizer::new("ifis x").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("ifis".to_string()),
                Token::Space,
                Token::RawString("x".to_string())
            ]
        );
    }

    #[test]
    fn test_if_then_else_predicate() {
        let tokens: Vec<Token> = Tokenizer::new("if ${x > 1} then 1 else 0").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::If,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("x".to_string()),
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::RawString("1".to_string()),
                Token::RCurly,
                Token::Space,
                Token::Then,
                Token::Space,
                Token::RawString("1".to_string()),
                Token::Space,
                Token::Else,
                Token::Space,
                Token::RawString("0".to_string()),
            ]
        );
    }

    #[test]
    fn test_if_then_else_multi_line() {
        let string = r#"
if ${x} then ${y}
else${z}
"#;

        let tokens: Vec<Token> = Tokenizer::new(string).run().value;

        assert_eq!(
            tokens,
            vec![
                Token::NewLine,
                Token::If,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("x".to_string()),
                Token::RCurly,
                Token::Space,
                Token::Then,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("y".to_string()),
                Token::RCurly,
                Token::NewLine,
                Token::Else,
                Token::InterpolationStart,
                Token::RawString("z".to_string()),
                Token::RCurly,
                Token::NewLine,
            ]
        );
    }

    #[test]
    fn test_if_then_else_false_expr() {
        let tokens: Vec<Token> = Tokenizer::new("ifxthenyelsez").run().value;

        assert_eq!(tokens, vec![Token::RawString("ifxthenyelsez".to_string()),]);
    }

    #[test]
    fn test_greater_than_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f >").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("f".to_string()),
                Token::Space,
                Token::GreaterThan,
            ]
        );
    }

    #[test]
    fn test_greater_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f  > g").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("f".to_string()),
                Token::Space,
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::RawString("g".to_string())
            ]
        );
    }

    #[test]
    fn test_greater_than_no_spaces() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}>${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::GreaterThan,
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_lessthan_partial() {
        let tokens: Vec<Token> = Tokenizer::new("f <").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("f".to_string()),
                Token::Space,
                Token::LessThan,
            ]
        );
    }

    #[test]
    fn test_less_than_with_space() {
        let tokens: Vec<Token> = Tokenizer::new("f < g").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("f".to_string()),
                Token::Space,
                Token::LessThan,
                Token::Space,
                Token::RawString("g".to_string())
            ]
        );
    }

    #[test]
    fn test_less_than_with_no_space() {
        let tokens: Vec<Token> = Tokenizer::new("f<g").run().value;

        assert_eq!(
            tokens,
            vec![
                Token::RawString("f".to_string()),
                Token::LessThan,
                Token::RawString("g".to_string())
            ]
        );
    }

    #[test]
    fn test_greater_than_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} > ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::Space,
                Token::GreaterThan,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_less_than_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} < ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::Space,
                Token::LessThan,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_equal_to_with_exprs() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} == ${bar}").run().value;

        //  let tokens: Vec<Token> = Tokenizer::new("{foo} raw {goo}").collect();

        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::Space,
                Token::EqualTo,
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning_and_end() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-raw_${bar}").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::RawString("-raw_".to_string()),
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_beginning() {
        let tokens: Vec<Token> = Tokenizer::new("${foo}-^raw").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::RawString("-^raw".to_string())
            ]
        );
    }

    #[test]
    fn test_with_place_holder_in_end() {
        let tokens: Vec<Token> = Tokenizer::new("raw ${foo}").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::RawString("raw".to_string()),
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
            ]
        );
    }

    #[test]
    fn test_with_place_holder_anywhere() {
        let tokens: Vec<Token> = Tokenizer::new("foo ${foo} raw ${bar} bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::RawString("foo".to_string()),
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::Space,
                Token::RawString("raw".to_string()),
                Token::Space,
                Token::InterpolationStart,
                Token::RawString("bar".to_string()),
                Token::RCurly,
                Token::Space,
                Token::RawString("bar".to_string())
            ]
        );
    }

    #[test]
    fn test_token_processing_with_dollar() {
        let tokens: Vec<Token> = Tokenizer::new("${foo} raw${hi} bar").run().value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::RawString("foo".to_string()),
                Token::RCurly,
                Token::Space,
                Token::RawString("raw".to_string()),
                Token::InterpolationStart,
                Token::RawString("hi".to_string()),
                Token::RCurly,
                Token::Space,
                Token::RawString("bar".to_string()),
            ]
        );
    }

    #[test]
    fn test_token_processing_with_match_expr() {
        let tokens: Vec<Token> = Tokenizer::new(
            "${match worker.response { some(value) => worker.response, none => 'some_value' } }",
        )
        .run()
        .value;
        assert_eq!(
            tokens,
            vec![
                Token::InterpolationStart,
                Token::Match,
                Token::Space,
                Token::Worker,
                Token::Dot,
                Token::RawString("response".to_string()),
                Token::Space,
                Token::LCurly,
                Token::Space,
                Token::Some,
                Token::LParen,
                Token::RawString("value".to_string()),
                Token::RParen,
                Token::Space,
                Token::Arrow,
                Token::Space,
                Token::Worker,
                Token::Dot,
                Token::RawString("response".to_string()),
                Token::Comma,
                Token::Space,
                Token::None,
                Token::Space,
                Token::Arrow,
                Token::Space,
                Token::Quote,
                Token::RawString("some_value".to_string()),
                Token::Quote,
                Token::Space,
                Token::RCurly,
                Token::Space,
                Token::RCurly,
            ]
        );
    }
}

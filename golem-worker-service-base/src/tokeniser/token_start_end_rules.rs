use crate::tokeniser::tokenizer::{MultiCharTokens, Token};
use std::collections::HashMap;

// We keep a set of rules where each rule is a pair of start and end token,
// that helps to find the start/end of a given token, or the possible block that a token can exist within.
// To handle all use-cases, we are going with the most granular approach instead
// of keeping a map of `end -> vec<start>` (or the other way around, may be for negligible performance benefits)
// to handle other subtle use-cases better.
// Example: For Token::Comma `,` we have a vec<start -> end> (they are `{  }`, `[  ]`, `( )` )
// that a comma can exist between each pair.
// Similarly, for `:` it is again vec<start -> end>, that it can exist between `{ -> }`
// However for `]`, it is `vec<start>`. This is because `]` can only be an end of `[`.
pub struct Rules(Vec<TokenStartEnd>);

impl Rules {
    pub fn find_starts_of_a_token(&self, token: &Token) -> Vec<Token> {
        let mut starts = vec![];
        for token_start_end in self.0.iter() {
            if &token_start_end.end == token {
                starts.push(token_start_end.start.clone());
            }
        }
        starts
    }

    pub fn as_hash_map(&self) -> HashMap<Token, Token> {
        let mut map = HashMap::new();
        for token_start_end in self.0.iter() {
            map.insert(token_start_end.start.clone(), token_start_end.clone().end);
        }
        map
    }

    pub fn all_token_starts(&self) -> Vec<Token> {
        let mut starts = vec![];
        for token_start_end in self.0.iter() {
            starts.push(token_start_end.start.clone());
        }

        starts
    }

    pub fn all_token_ends(&self) -> Vec<Token> {
        let mut ends = vec![];
        for token_start_end in self.0.iter() {
            ends.push(token_start_end.end.clone());
        }

        ends
    }
    pub fn of_token(end_token: &Token) -> Rules {
        let vec = match end_token {
            // `}` can exist as an end of another `{` or `${`
            Token::RCurly => vec![
                TokenStartEnd::of_lcurly(),
                TokenStartEnd::of_code_interpolation(),
            ],
            // `]` can exist as an end of another `[`
            Token::RSquare => vec![TokenStartEnd::of_lsquare()],
            // `)` can exist as an end of another `(`
            Token::RParen => vec![TokenStartEnd::of_lparen()],
            // A `,` can exist within another `[`, or `{`, or `(`
            Token::Comma => vec![
                TokenStartEnd::of_lparen(),
                TokenStartEnd::of_lcurly(),
                TokenStartEnd::of_lsquare(),
                TokenStartEnd::of_code_interpolation(),
            ],
            // A `:` can exist within another `{`
            Token::Colon => vec![TokenStartEnd::of_lcurly()],
            Token::SemiColon => vec![],
            Token::WildCard => vec![],
            Token::At => vec![],
            Token::Escape => vec![],
            Token::MultiChar(multi) => {
                match multi {
                    MultiCharTokens::Ok => vec![],      // hardly act as an end token
                    MultiCharTokens::Err => vec![],     // hardly act as an end token
                    MultiCharTokens::Some => vec![],    // hardly act as an end token
                    MultiCharTokens::None => vec![],    // hardly act as an end token
                    MultiCharTokens::Match => vec![],   // hardly act as an end token
                    MultiCharTokens::InterpolationStart => vec![], // hardly act as an end token
                    MultiCharTokens::GreaterThanOrEqualTo => vec![], // hardly act as an end token
                    MultiCharTokens::LessThanOrEqualTo => vec![], // hardly act as an end token
                    MultiCharTokens::EqualTo => vec![], // hardly act as an end token
                    MultiCharTokens::If => vec![],      // hardly act as an end token
                    MultiCharTokens::Then => vec![TokenStartEnd::of_if()],
                    MultiCharTokens::Else => vec![TokenStartEnd::of_then()],
                    MultiCharTokens::Arrow => vec![], // hardly act as an end token
                    MultiCharTokens::Let => vec![],   // hardly act as an end token
                    MultiCharTokens::NumberLiteral(_) => vec![], // hardly act as an end token
                    MultiCharTokens::StringLiteral(_) => vec![], // hardly act as an end token
                    MultiCharTokens::Identifier(_) => vec![],
                    MultiCharTokens::BooleanLiteral(_) => vec![], // hardly act as an end token
                }
            }
            Token::LCurly => vec![],
            Token::LSquare => vec![],
            Token::GreaterThan => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::LessThan => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::Space => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::NewLine => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::LParen => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::Dot => vec![],
            Token::Quote => vec![], // Not acting as an end of another expression, we hardly use this as an end token
            Token::LetEqual => vec![], // Not acting as an end of another expression, we hardly use this as an end token
        };

        Rules(vec)
    }
}

// A generic set of rules of start and end tokens
// Also helps to find situations where a token can exist between a start and end token
#[derive(Debug, Clone)]
pub struct TokenStartEnd {
    pub start: Token,
    pub end: Token,
}

impl TokenStartEnd {
    pub fn new(start: Token, ends: Token) -> Self {
        TokenStartEnd { start, end: ends }
    }

    pub fn of_code_interpolation() -> TokenStartEnd {
        TokenStartEnd::new(
            Token::MultiChar(MultiCharTokens::InterpolationStart),
            Token::RCurly,
        )
    }

    pub fn of_lcurly() -> TokenStartEnd {
        TokenStartEnd::new(Token::LCurly, Token::RCurly)
    }

    pub fn of_lsquare() -> TokenStartEnd {
        TokenStartEnd::new(Token::LSquare, Token::RSquare)
    }

    pub fn of_lparen() -> TokenStartEnd {
        TokenStartEnd::new(Token::LParen, Token::RParen)
    }

    pub fn of_let() -> TokenStartEnd {
        TokenStartEnd::new(Token::MultiChar(MultiCharTokens::Let), Token::SemiColon)
    }

    pub fn of_if() -> TokenStartEnd {
        TokenStartEnd::new(
            Token::MultiChar(MultiCharTokens::If),
            Token::MultiChar(MultiCharTokens::Then),
        )
    }

    pub fn of_then() -> TokenStartEnd {
        TokenStartEnd::new(
            Token::MultiChar(MultiCharTokens::Then),
            Token::MultiChar(MultiCharTokens::Else),
        )
    }
}

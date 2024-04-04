use super::tokenizer::{Token, Tokenizer};

// A simple wrapper over tokeniser traversing back and forth easily
pub struct TokenCursor<'a> {
    pub(crate) current_token: Option<Token>,
    pub(crate) tokenizer: Tokenizer<'a>,
}

impl<'a> TokenCursor<'a> {
    pub fn new(tokens: &'a str) -> TokenCursor<'a> {
        let str = Tokenizer::new(tokens);

        TokenCursor {
            current_token: None,
            tokenizer: str
        }
    }

    pub fn current_token (&self) -> Option<Token> {
        self.current_token.clone()
    }

    pub fn peek(&mut self) -> Option<Token> {
        if let Some(token) = self.tokenizer.next_token() {
            self.tokenizer.state.pos -= token.to_string().len();
            Some(token)
        } else {
            None
        }
    }

    pub fn peek_at(&mut self, index: usize) -> Option<Token> {
        let original_state = self.tokenizer.state.pos;
        let original_token = self.current_token.clone();
        self.tokenizer.state.pos = index;

        if let Some(token) = self.tokenizer.next_token() {
            self.tokenizer.state.pos = original_state;
            self.current_token = original_token;
            Some(token)
        } else {
            None
        }
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.tokenizer.next_token()
    }


    pub fn next_non_empty_token(&mut self) -> Option<Token> {
        self.skip_whitespace()
    }

    // Captures the string upto the end token, and advance the cursor further skipping the end token
    pub fn capture_string_until_and_skip_end(
        &mut self,
        start: Vec<&Token>,
        end: &Token,
    ) -> Option<String> {
        let captured_string = self.capture_string_until(start, end);
        match captured_string {
            Some(captured_string) => {
                self.next_token();
                Some(captured_string)
            }
            None => None,
        }
    }
    // Captures the string upto the end token, leaving the cursor at the end token (leaving it to the user)
    pub fn capture_string_until(&mut self, start: Vec<&Token>, end: &Token) -> Option<String> {
        let capture_until = self.index_of_last_end_token(start, end)?;

        let tokens = self.tokenizer.all_tokens_until(capture_until);

        Some(tokens
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<String>>()
            .join("")
        )
    }

    pub fn capture_tail(&mut self) -> Option<String> {
        // Skip head
        self.tokenizer.next_token();

        let str = self.tokenizer.rest().to_string();

        if str.is_empty() {
            None
        } else {
            Some(str)
        }
    }

    // This is useful especially when we want to capture string between two tokens,
    // If the start token repeats again (nested start), then it looks for
    // the end token for the inner start, and repeats until it find
    // the end token for the outer start. Here start corresponding to a particular end
    // can be n number of tokens. Example `${` can be the start of `}` or `{` can be the start of `}`.
    pub fn index_of_last_end_token(
        &mut self,
        nested_starts: Vec<&Token>,
        end: &Token,
    ) -> Option<usize> {
        let starts = nested_starts
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<String>>();
        let mut index: usize = self.tokenizer.state.pos;

        let mut start_token_count = 0;

        let mut found: bool = false;

        while let Some(current_token) = self.peek_at(index).map(|x| x.to_string()) {
            if starts.contains(&current_token) {
                // That it finds a start token again
                start_token_count += 1;
            } else if current_token == end.to_string() {
                // Making sure any nested start token was closed (making it always a zero) before break
                if start_token_count == 0 {
                    // Found a matching end token
                    found = true;
                    break;
                } else {
                    // Implies end for nested happened
                    start_token_count -= 1;
                }
            }

            index += current_token.len() ;
        }

        if found {
            Some(index)
        } else {
            None
        }
    }

    pub fn skip_whitespace(&mut self) -> Option<Token> {
        let mut non_empty_token: Option<Token> = None;
        while let Some(token) = self.next_token() {
            if token.is_empty() {
            } else {
                non_empty_token = Some(token);
                break;
            }
        }

        non_empty_token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_string_test() {
        let tokens = "(afsal)";

        let mut cursor = TokenCursor::new(tokens);
        cursor.next_token();
        let result = cursor
            .capture_string_until(vec![&Token::LParen], &Token::RParen)
            .unwrap();

        assert_eq!(result, "afsal".to_string())
    }

    #[test]
    fn capture_string_test_nested() {
        let tokens = "((afsal))";

        let mut cursor = TokenCursor::new(tokens);
        cursor.next_token();
        let result = cursor
            .capture_string_until(vec![&Token::LParen], &Token::RParen)
            .unwrap();

        assert_eq!(result, "(afsal)")
    }

    #[test]
    fn capture_character_test() {
        let tokens = ")";

        let mut cursor = TokenCursor::new(tokens);
        let result = cursor
            .capture_string_until(vec![&Token::LParen], &Token::RParen)
            .unwrap();
        assert_eq!(result, "".to_string())
    }

    #[test]
    fn capture_empty_test() {
        let tokens = "";

        let mut cursor = TokenCursor::new(tokens);
        let result = cursor.capture_string_until(vec![&Token::LParen], &Token::RParen);

        assert_eq!(result, None)
    }

    #[test]
    fn test_capture_string_from() {
        let tokens = "else foo";

        let mut cursor = TokenCursor::new(tokens);
        let result = cursor.capture_tail();

        assert_eq!(result, Some(" foo".to_string()))
    }

    #[test]
    fn test_index_of_last_end_token() {
        let tokens = "else foo }";

        let mut cursor = TokenCursor::new(tokens);
        let result = cursor.index_of_last_end_token(vec![&Token::LCurly], &Token::RCurly );

        assert_eq!(result, Some(10))
    }
}

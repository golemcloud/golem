use std::collections::HashMap;
use std::fmt::Display;

use bincode::{Decode, Encode};
use golem_service_base::model::Type;
use golem_wasm_rpc::TypeAnnotatedValue;
use hyper::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tokeniser::cursor::TokenCursor;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Data that represent the resolved variables
// Values are often resolved from input request, or output response of a worker, both of which are JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedVariables {
    pub variables: TypeAnnotatedValue,
}

impl Default for ResolvedVariables {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct Path(pub Vec<PathComponent>);

impl Path {
    pub fn from_key(input: &str) -> Path {
        let mut path = Path::new();
        path.update_key(input);
        path
    }

    pub fn from_index(index: usize) -> Path {
        let mut path = Path::new();
        path.update_index(index);
        path
    }
}

pub struct RemainingPath(Path);

impl RemainingPath {
    pub fn drop_path(&self, path: &Path) -> Option<RemainingPath> {
        self.0.drop_path(path)
    }

    pub fn get_index(&self) -> Option<&Index> {
        let first = self.0 .0.first()?;
        first.get_index()
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn all_components(&self) -> &Vec<PathComponent> {
        &self.path().0
    }
}

impl Path {
    pub fn starts_with(&self, path: &Path) -> bool {
        self.0.starts_with(&path.0)
    }

    pub fn get_index(&self) -> Option<&Index> {
        let first = self.0.first()?;

        match first {
            PathComponent::Index(index) => Some(index),
            _ => None,
        }
    }
    pub fn drop_path(&self, path: &Path) -> Option<RemainingPath> {
        if self.0.starts_with(&path.0) {
            let mut result = self.0.clone();

            for item in &path.0 {
                if let Some(pos) = result.iter().position(|x| x.clone() == item.clone()) {
                    result.drain(0..=pos);
                }
            }

            if result.is_empty() {
                None
            } else {
                Some(RemainingPath(Path(result)))
            }
        } else {
            None
        }
    }

    pub fn from_raw_string(input: &str) -> Path {
        let mut path = Path::new();
        path.update_key(input);
        path
    }

    // For strings that are static and can never fail and make use of it
    pub fn from_string_unsafe(input: &str) -> Path {
        Path::from_string(input).unwrap()
    }

    pub fn from_string(input: &str) -> Result<Path, String> {
        let tokens: Vec<Token> = Tokenizer::new(input).collect();
        let mut cursor = TokenCursor::new(tokens);
        let mut path = Path::new();

        while let Some(token) = cursor.next_non_empty_token() {
            match token {
                Token::OpenSquareBracket => {
                    let probable_index = cursor.capture_string_until(
                        vec![&Token::OpenSquareBracket],
                        &Token::ClosedSquareBracket,
                    );

                    match probable_index {
                        Some(index) => {
                            if let Ok(index) = index.parse::<usize>() {
                                path.update_index(index);
                            } else {
                                return Err("Invalid path".to_string());
                            }
                        }
                        None => {
                            return Err(
                                format!("Failed to parse path {}. Expecting a closed bracket ] corresponding to an open bracket [", input)
                            )
                        }
                    }
                }

                token => {
                    let token_string = token.to_string();
                    let dot_separated_keys: Vec<&str> = token_string.split('.').collect();

                    for key in dot_separated_keys {
                        path.update_key(key)
                    }
                }
            }
        }

        Ok(path)
    }

    pub fn new() -> Path {
        Path(vec![])
    }

    pub fn update_key(&mut self, input: &str) {
        self.0.push(PathComponent::key_name(input));
    }

    pub fn update_index(&mut self, index: usize) {
        self.0.push(PathComponent::index(index));
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut path = "".to_string();

        for p in self.0.clone() {
            match p {
                PathComponent::Index(index) => path.push_str(format!("[{}]", index.0).as_str()),
                PathComponent::KeyName(keyname) => {
                    path.push_str(format!(".{}", keyname.0).as_str())
                }
            }
        }

        write!(f, "{}", path.trim_start_matches('.'))
    }
}

impl Iterator for Path {
    type Item = PathComponent;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((head, tail)) = self.0.clone().split_first() {
            self.0 = tail.to_vec();
            Some(head.clone())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct Index(pub usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct KeyName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub enum PathComponent {
    Index(Index),
    KeyName(KeyName),
}

impl PathComponent {

    fn key(input: &str) -> PathComponent {
        PathComponent::KeyName(KeyName(input.to_string()))
    }

    fn ind(index: usize) -> PathComponent {
        PathComponent::Index(Index(index))
    }

    fn get_index(&self) -> Option<&Index> {
        match self {
            PathComponent::KeyName(_) => None,
            PathComponent::Index(index) => Some(index),
        }
    }

    fn key_name(input: &str) -> PathComponent {
        PathComponent::KeyName(KeyName(input.to_string()))
    }

    fn index(index: usize) -> PathComponent {
        PathComponent::Index(Index(index))
    }
}

use std::collections::HashMap;
use std::fmt::Display;

use bincode::{Decode, Encode};
use hyper::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tokeniser::cursor::TokenCursor;
use crate::tokeniser::tokenizer::{Token, Tokenizer};
use crate::worker_request_to_http::WorkerResponse;

// Data that represent the resolved variables
// Values are often resolved from input request, or output response of a worker, both of which are JSON.
#[derive(Debug, Clone)]
pub struct ResolvedVariables {
    pub variables: HashMap<Path, Value>,
}

impl Default for ResolvedVariables {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolvedVariables {
    pub fn new() -> ResolvedVariables {
        ResolvedVariables {
            variables: HashMap::new(),
        }
    }

    pub fn from_worker_response(worker_response: &WorkerResponse) -> ResolvedVariables {
        let mut vars: ResolvedVariables = ResolvedVariables::new();
        let path = Path::from_string_unsafe(Token::WorkerResponse.to_string().as_str());

        vars.insert(path, worker_response.result.clone());

        vars
    }

    pub fn from_http_request(
        request_body: &Value,
        request_header: &HeaderMap,
        request_query_variables: HashMap<String, String>,
        spec_query_variables: Vec<String>,
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<ResolvedVariables, Vec<String>> {
        let mut gateway_variables = ResolvedVariables::new();

        let mut headers: serde_json::Map<String, Value> = serde_json::Map::new();

        for (header_name, header_value) in request_header {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            headers.insert(
                header_name.to_string(),
                Value::String(header_value_str.to_string()),
            );
        }

        let request_headers = Value::Object(headers.clone());
        let mut request_query_values = ResolvedVariables::get_request_query_values(
            request_query_variables,
            spec_query_variables,
        )?;

        let request_path_values =
            ResolvedVariables::get_request_path_values(request_path_values, spec_path_variables)?;

        request_query_values.extend(request_path_values);

        let mut request_details = serde_json::Map::new();
        request_details.insert("body".to_string(), request_body.clone());
        request_details.insert("header".to_string(), request_headers);
        request_details.insert("path".to_string(), Value::Object(request_query_values));

        gateway_variables.insert(
            Path::from_string_unsafe(Token::Request.to_string().as_str()),
            Value::Object(request_details),
        );

        Ok(gateway_variables)
    }

    fn get_request_path_values(
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<serde_json::Map<String, Value>, Vec<String>> {
        let mut unavailable_path_variables: Vec<String> = vec![];
        let mut path_variables_map = serde_json::Map::new();

        for (index, spec_path_variable) in spec_path_variables.iter() {
            if let Some(path_value) = request_path_values.get(index) {
                path_variables_map.insert(
                    spec_path_variable.clone(),
                    serde_json::Value::String(path_value.trim().to_string()),
                );
            } else {
                unavailable_path_variables.push(spec_path_variable.to_string());
            }
        }

        if unavailable_path_variables.is_empty() {
            Ok(path_variables_map)
        } else {
            Err(unavailable_path_variables)
        }
    }

    fn get_request_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: Vec<String>,
    ) -> Result<serde_json::Map<String, Value>, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map = serde_json::Map::new();

        for spec_query_variable in spec_query_variables.iter() {
            if let Some(query_value) = request_query_variables.get(spec_query_variable) {
                query_variable_map.insert(
                    spec_query_variable.clone(),
                    serde_json::Value::String(query_value.trim().to_string()),
                );
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            Ok(query_variable_map)
        } else {
            Err(unavailable_query_variables)
        }
    }

    pub fn extend(&mut self, that: &ResolvedVariables) {
        self.variables.extend(that.variables.clone());
    }

    pub fn insert_primitives(&mut self, key: &str, value: &str) {
        let path = Path(vec![PathComponent::key_name(key)]);
        let value = Value::String(value.to_string());

        self.variables.insert(path, value);
    }

    pub fn insert(&mut self, key: Path, value: Value) {
        self.variables.insert(key, value);
    }

    pub fn get_path(&self, input_path: &Path) -> Option<Value> {
        if let Some(value) = self.variables.get(input_path) {
            Some(value.clone())
        } else {
            // Try to go deeper in the tree and fetch the value
            fn go(json_value: &Value, path: &[PathComponent]) -> Option<Value> {
                if let Some((next_pc, tail)) = path.split_first() {
                    let path_tail: Vec<PathComponent> = tail.to_vec();

                    let optional_json_result = match json_value {
                        Value::Object(map) => get_from_map(map, next_pc),
                        Value::Array(sequence) => get_from_sequence(sequence, next_pc),
                        Value::Null => Some(Value::Null),
                        Value::Bool(_) => None,
                        Value::Number(_) => None,
                        Value::String(_) => None,
                    };

                    if let Some(json_result) = optional_json_result {
                        if path_tail.is_empty() {
                            Some(json_result)
                        } else {
                            go(&json_result, &path_tail)
                        }
                    } else {
                        None
                    }
                } else {
                    Some(json_value.clone())
                }
            }

            let mut result_value: Option<Value> = None;

            for (existing_path, existing_value) in self.variables.clone() {
                if input_path.starts_with(&existing_path) {
                    let remaining_path = input_path.drop_path(&existing_path);

                    if let Some(remaining_path) = remaining_path {
                        result_value = go(&existing_value, &remaining_path.path().0);
                    }
                }
            }

            result_value
        }
    }

    pub fn get_key(&self, string: &str) -> Option<&Value> {
        self.variables
            .get(&Path(vec![PathComponent::key_name(string)]))
    }
}

fn get_from_map(
    map: &serde_json::Map<String, Value>,
    path_component: &PathComponent,
) -> Option<Value> {
    match path_component {
        PathComponent::KeyName(key_name) => map.get(&key_name.0).cloned(),
        PathComponent::Index(_) => None,
    }
}

fn get_from_sequence(sequence: &[Value], path_component: &PathComponent) -> Option<Value> {
    match path_component {
        PathComponent::Index(index) => {
            if index.0 < sequence.len() {
                let value = &sequence[index.0];
                Some(value.clone())
            } else {
                None
            }
        }

        PathComponent::KeyName(_) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct Path(pub Vec<PathComponent>);

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
                    let probable_index = cursor.capture_string_between(
                        &Token::OpenSquareBracket,
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

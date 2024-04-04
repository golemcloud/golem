use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::str::FromStr;
use Iterator;

use bincode::{Decode, Encode};
use derive_more::Display;
use poem_openapi::Enum;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::api_definition::{
    ApiDefinitionId, ApiVersion, HasApiDefinitionId, HasGolemWorkerBindings, HasVersion,
};
use crate::parser::path_pattern_parser::PathPatternParser;
use crate::parser::{GolemParser, ParseError};
use crate::worker_binding::GolemWorkerBinding;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDefinition {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<Route>,
}

impl HasGolemWorkerBindings for HttpApiDefinition {
    fn get_golem_worker_bindings(&self) -> Vec<GolemWorkerBinding> {
        self.routes
            .iter()
            .map(|route| route.binding.clone())
            .collect()
    }
}

impl HasApiDefinitionId for HttpApiDefinition {
    fn get_api_definition_id(&self) -> ApiDefinitionId {
        self.id.clone()
    }
}

impl HasVersion for HttpApiDefinition {
    fn get_version(&self) -> ApiVersion {
        self.version.clone()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Display, Encode, Decode, Enum,
)]
pub enum MethodPattern {
    Get,
    Connect,
    Post,
    Delete,
    Put,
    Patch,
    Options,
    Trace,
    Head,
}

impl MethodPattern {
    pub fn is_connect(&self) -> bool {
        matches!(self, MethodPattern::Connect)
    }

    pub fn is_delete(&self) -> bool {
        matches!(self, MethodPattern::Delete)
    }

    pub fn is_get(&self) -> bool {
        matches!(self, MethodPattern::Get)
    }

    pub fn is_head(&self) -> bool {
        matches!(self, MethodPattern::Head)
    }
    pub fn is_post(&self) -> bool {
        matches!(self, MethodPattern::Post)
    }

    pub fn is_put(&self) -> bool {
        matches!(self, MethodPattern::Put)
    }

    pub fn is_options(&self) -> bool {
        matches!(self, MethodPattern::Options)
    }

    pub fn is_patch(&self) -> bool {
        matches!(self, MethodPattern::Patch)
    }

    pub fn is_trace(&self) -> bool {
        matches!(self, MethodPattern::Trace)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct LiteralInfo(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct VarInfo {
    pub key_name: String,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct QueryInfo {
    pub key_name: String,
}

impl Display for QueryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{{}}}", self.key_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct AllPathPatterns {
    pub path_patterns: Vec<PathPattern>,
    pub query_params: Vec<QueryInfo>,
}

impl AllPathPatterns {
    pub fn get_path_variables(&self) -> HashMap<usize, String> {
        let mut result: HashMap<usize, String> = HashMap::new();

        for path_pattern in self.path_patterns.iter() {
            if let PathPattern::Var(var_info) = path_pattern {
                result.insert(var_info.index, var_info.key_name.clone());
            }
        }

        result
    }

    pub fn get_path_literals(&self) -> HashMap<usize, String> {
        let mut result: HashMap<usize, String> = HashMap::new();

        for (index, path_pattern) in self.path_patterns.iter().enumerate() {
            if let PathPattern::Literal(literal_info) = path_pattern {
                result.insert(index, literal_info.0.clone());
            }
        }

        result
    }

    pub fn get_query_variables(&self) -> Vec<String> {
        self.query_params
            .iter()
            .map(|x| x.key_name.clone())
            .collect()
    }

    pub fn get_path_prefixed_variables(&self) -> HashSet<String> {
        let mut result: HashSet<String> = HashSet::new();
        for (_, v) in self.get_path_variables() {
            result.insert(format!("request.path.{}", v.trim()));
        }
        for v in self.get_query_variables() {
            result.insert(format!("request.path.{}", v.trim()));
        }
        result
    }

    pub fn parse(input: &str) -> Result<AllPathPatterns, ParseError> {
        input.parse()
    }
}

impl Display for AllPathPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, pattern) in self.path_patterns.iter().enumerate() {
            if index > 0 {
                write!(f, "{}", '/')?;
            }
            write!(f, "{}", pattern)?;
        }

        if !self.query_params.is_empty() {
            write!(f, "{}", '?')?;
            for (index, query) in self.query_params.iter().enumerate() {
                if index > 0 {
                    write!(f, "{}", '&')?;
                }
                write!(f, "{}", query)?;
            }
        }

        Ok(())
    }
}

impl FromStr for AllPathPatterns {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PathPatternParser.parse(s)
    }
}

impl<'de> Deserialize<'de> for AllPathPatterns {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value {
            Value::String(value) => match AllPathPatterns::parse(value.as_str()) {
                Ok(path_pattern) => Ok(path_pattern),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            _ => Err(serde::de::Error::custom("Failed to parse path from yaml")),
        }
    }
}

impl Serialize for AllPathPatterns {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = Value::String(self.to_string());
        Value::serialize(&value, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub enum PathPattern {
    Literal(LiteralInfo),
    Var(VarInfo),
}

impl PathPattern {
    pub fn literal(value: impl Into<String>) -> PathPattern {
        PathPattern::Literal(LiteralInfo(value.into()))
    }

    pub fn var(value: impl Into<String>, index: usize) -> PathPattern {
        PathPattern::Var(VarInfo {
            key_name: value.into(),
            index,
        })
    }
}

impl Display for PathPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathPattern::Literal(info) => write!(f, "{}", info.0),
            PathPattern::Var(info) => write!(f, "{{{}}}", info.key_name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Route {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub binding: GolemWorkerBinding,
}

#[cfg(test)]
mod tests {
    use golem_common::serialization;

    use crate::expression::Expr;

    use super::*;

    #[test]
    fn split_path_works_with_single_value() {
        let path_pattern = "foo";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![PathPattern::literal("foo")],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_multiple_values() {
        let path_pattern = "foo/bar";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![PathPattern::literal("foo"), PathPattern::literal("bar")],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_variables() {
        let path_pattern = "foo/bar/{var}";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![
                PathPattern::literal("foo"),
                PathPattern::literal("bar"),
                PathPattern::var("var", 2),
            ],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_variables_and_queries() {
        let path_pattern = "foo/bar/{var}?{userid1}&{userid2}";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![
                PathPattern::literal("foo"),
                PathPattern::literal("bar"),
                PathPattern::var("var", 2),
            ],
            query_params: vec![
                QueryInfo {
                    key_name: "userid1".to_string(),
                },
                QueryInfo {
                    key_name: "userid2".to_string(),
                },
            ],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn get_path_variables_as_map() {
        let path_pattern_str = "foo / bar / {var1} / {var2} ? {userid1} & {userid2}";
        let path_pattern = AllPathPatterns::parse(path_pattern_str).unwrap();

        let path_variables_map = path_pattern.get_path_variables();

        let mut expected: HashMap<usize, String> = HashMap::new();
        expected.insert(3, "var2".to_string());
        expected.insert(2, "var1".to_string());

        assert_eq!(path_variables_map, expected);
    }

    #[test]
    fn get_query_variables() {
        let path_pattern_str = "foo/bar/{var1}/{var2}?{userid1}&{userid2}";
        let path_pattern = AllPathPatterns::parse(path_pattern_str).unwrap();

        let query_variables_map = path_pattern.get_query_variables();

        let expected = vec!["userid1".to_string(), "userid2".to_string()];

        assert_eq!(query_variables_map, expected);
    }

    #[track_caller]
    fn test_path_pattern_to_string(path_pattern_str: &str) {
        let path_pattern = AllPathPatterns::parse(path_pattern_str).unwrap();
        let path_pattern_str_result = path_pattern.to_string();
        assert_eq!(
            path_pattern_str_result,
            path_pattern_str,
            "Assertion failed for test case at {}",
            std::panic::Location::caller()
        );
    }

    #[test]
    fn test_path_patterns_to_string() {
        test_path_pattern_to_string("foo/bar/{var1}/{var2}?{userid1}&{userid2}");
        test_path_pattern_to_string("foo/bar/{var1}/{var2}?{userid1}");
        test_path_pattern_to_string("foo/bar/{var1}/{var2}");
        test_path_pattern_to_string("foo/bar");
    }

    #[track_caller]
    fn test_string_expr_parse_and_encode(input: &str) {
        let parsed_expr1 = Expr::from_primitive_string(input).unwrap();
        let encoded_expr = parsed_expr1.to_string().unwrap();
        let parsed_expr2 = Expr::from_primitive_string(encoded_expr.as_str()).unwrap();

        assert_eq!(
            parsed_expr1,
            parsed_expr2,
            "Assertion failed for test case at {}",
            std::panic::Location::caller()
        );
    }

    #[test]
    fn expr_parser_without_vars() {
        test_string_expr_parse_and_encode("foo");
    }

    #[test]
    fn expr_parser_with_vars() {
        test_string_expr_parse_and_encode("worker-id-${request.path.user_id}");
    }

    #[test]
    fn expression_with_predicate0() {
        test_string_expr_parse_and_encode("1>2");
    }

    #[test]
    fn expression_with_predicate1() {
        test_string_expr_parse_and_encode("${request.path.user-id > request.path.id}");
    }

    #[test]
    fn expression_with_predicate2() {
        test_string_expr_parse_and_encode("${request.path.user-id}>2");
    }

    #[test]
    fn expression_with_predicate3() {
        test_string_expr_parse_and_encode("${request.path.user-id}=2");
    }

    #[test]
    fn expression_with_predicate4() {
        test_string_expr_parse_and_encode("${request.path.user-id}<2");
    }

    #[test]
    fn expr_with_if_condition() {
        test_string_expr_parse_and_encode("${if (request.path.user_id>1) then 1 else 0}");
    }

    #[test]
    fn expr_with_if_condition_with_expr_left() {
        test_string_expr_parse_and_encode(
            "${if (request.path.user_id>1) then request.path.user_id else 0}",
        );
    }

    #[test]
    fn expr_with_if_condition_with_expr_left_right() {
        test_string_expr_parse_and_encode(
            "${if (request.path.user_id>1) then request.path.user_id else request.path.id}",
        );
    }

    #[test]
    fn expr_with_if_condition_with_expr_right() {
        test_string_expr_parse_and_encode(
            "${if (request.path.user_id>1) then 0 else request.path.id}",
        );
    }

    #[test]
    fn expr_with_if_condition_with_with_literals() {
        test_string_expr_parse_and_encode(
            "foo-${if (request.path.user_id>1) then request.path.user_id else 0}",
        );
    }

    #[test]
    fn expr_request() {
        test_string_expr_parse_and_encode("${request}");
    }

    #[test]
    fn expr_worker_response() {
        test_string_expr_parse_and_encode("${worker.response}");
    }

    // TODO; Avoid having to pass null to fix tests
    fn get_api_spec(
        path_pattern: &str,
        worker_id: &str,
        function_params: &str,
    ) -> serde_yaml::Value {
        let yaml_string = format!(
            r#"
          id: users-api
          version: 0.0.1
          projectId: '15d70aa5-2e23-4ee3-b65c-4e1d702836a3'
          routes:
          - method: Get
            path: {}
            binding:
              template: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerId: '{}'
              functionName: golem:it/api/get-cart-contents
              functionParams: {}
              response:
                status: '{}'
                body: '{}'
                headers:
                  user: '{}'

        "#,
            path_pattern,
            worker_id,
            function_params,
            "${if (worker.response.user == \"admin\") then 401 else 200}",
            "hello-${if (worker.response.user == \"admin\") then \"unauthorised\" else ${worker.response.user}}",
            "hello-${worker.response.user}"
        );

        let de = serde_yaml::Deserializer::from_str(yaml_string.as_str());
        serde_yaml::Value::deserialize(de).unwrap()
    }

    #[test]
    fn test_api_spec_serde() {
        test_serde(
            "foo/{user-id}?{id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body}\"]",
        );

        test_serde(
            "foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
        );

        test_serde(
            "foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.path.user-id}\"]",
        );

        test_serde(
            "foo",
            "shopping-cart-${if (${request.body.user-id}>100) then 0 else 1}",
            "[ \"data\"]",
        );
    }

    #[track_caller]
    fn test_serde(path_pattern: &str, worker_id: &str, function_params: &str) {
        let yaml = get_api_spec(path_pattern, worker_id, function_params);
        let result: HttpApiDefinition = serde_yaml::from_value(yaml.clone()).unwrap();
        let yaml2 = serde_yaml::to_value(result.clone()).unwrap();
        let result2: HttpApiDefinition = serde_yaml::from_value(yaml2.clone()).unwrap();

        assert_eq!(
            result,
            result2,
            "Assertion failed for test case at {}",
            std::panic::Location::caller()
        );
    }

    #[test]
    fn test_api_spec_encode_decode() {
        fn test_encode_decode(path_pattern: &str, worker_id: &str, function_params: &str) {
            let yaml = get_api_spec(path_pattern, worker_id, function_params);
            let original: HttpApiDefinition = serde_yaml::from_value(yaml.clone()).unwrap();
            let encoded = serialization::serialize(&original).unwrap();
            let decoded: HttpApiDefinition = serialization::deserialize(&encoded).unwrap();

            assert_eq!(original, decoded);
        }

        test_encode_decode(
            "foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body}\"]",
        );

        test_encode_decode(
            "foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
        );

        test_encode_decode(
            "foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.path.user-id}\"]",
        );

        test_encode_decode(
            "foo",
            "shopping-cart-${if (${request.body.user-id}>100) then 0 else 1}",
            "[ \"data\"]",
        );

        test_encode_decode(
            "foo",
            "match worker.response { ok(value) => 1, error => 0 }",
            "[ \"data\"]",
        );
    }
}

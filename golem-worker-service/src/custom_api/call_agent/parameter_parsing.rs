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

use crate::custom_api::error::RequestHandlerError;
use golem_service_base::custom_api::parameter_parsing::{SCALAR_MAP_ERROR, map_entry_key};

pub(in crate::custom_api) fn validate_query_parameters(
    request: &super::RichRequest,
    behaviour: &golem_service_base::custom_api::CallAgentBehaviour,
    controls: &[&str],
) -> Result<(), RequestHandlerError> {
    use golem_service_base::custom_api::{MethodParameter, PhantomSelection};
    let mut names: Vec<_> = request.query_params().keys().collect();
    names.sort_unstable();
    for name in names {
        if controls.contains(&name.as_str())
            || matches!(&behaviour.phantom_selection, PhantomSelection::Query { name: selector, .. } if selector == name)
        {
            continue;
        }
        if let Some(MethodParameter::Query { argument_name, query_parameter_name, parameter_type }) =
            behaviour.method_parameters.iter().find(|parameter| {
                matches!(parameter, MethodParameter::Query { query_parameter_name, parameter_type, .. }
                if query_parameter_name == name
                    || (parameter_type.is_map() && map_entry_key(query_parameter_name, name).is_some()))
            })
        {
            if parameter_type.is_map() && query_parameter_name == name {
                return Err(RequestHandlerError::InvalidParameter {
                    name: argument_name.clone(),
                    error: SCALAR_MAP_ERROR.into(),
                });
            }
            continue;
        }
        return Err(RequestHandlerError::UnknownQueryParameter { name: name.clone() });
    }
    Ok(())
}

#[cfg(test)]
mod path_segment_tests {
    use assert2::assert;
    use golem_common::schema::SchemaValue;
    use golem_service_base::custom_api::PathSegmentType;
    use golem_service_base::custom_api::parameter_parsing::{
        HttpParameterError, parse_path_segment_value,
    };
    use test_r::test;

    #[test]
    fn parse_string_path_segment() {
        let result = parse_path_segment_value("hello".to_string(), &PathSegmentType::Str).unwrap();

        assert_eq!(result, SchemaValue::String("hello".into()));
    }

    #[test]
    fn parse_char_success() {
        let result = parse_path_segment_value("a".to_string(), &PathSegmentType::Chr).unwrap();

        assert_eq!(result, SchemaValue::Char('a'));
    }

    #[test]
    fn parse_char_failure_multiple_chars() {
        let err = parse_path_segment_value("ab".to_string(), &PathSegmentType::Chr).unwrap_err();

        assert!(let HttpParameterError::ValueParsingFailed {
            expected: "char",
            ..
        } = err);
    }

    #[test]
    fn parse_numeric_success() {
        let cases = vec![
            (PathSegmentType::U8, "12", SchemaValue::U8(12)),
            (PathSegmentType::S16, "-5", SchemaValue::S16(-5)),
            (PathSegmentType::U32, "42", SchemaValue::U32(42)),
            (PathSegmentType::S64, "-100", SchemaValue::S64(-100)),
            (PathSegmentType::F32, "1.5", SchemaValue::F32(1.5)),
            (PathSegmentType::F64, "2.25", SchemaValue::F64(2.25)),
        ];

        for (ty, input, expected) in cases {
            let value = parse_path_segment_value(input.to_string(), &ty).unwrap();

            assert_eq!(value, expected);
        }
    }

    #[test]
    fn parse_numeric_failure() {
        let err = parse_path_segment_value("not-a-number".to_string(), &PathSegmentType::U32)
            .unwrap_err();

        assert!(let HttpParameterError::ValueParsingFailed {
            expected: "u32",
            ..
        } = err);
    }

    #[test]
    fn parse_bool_success() {
        let value = parse_path_segment_value("true".to_string(), &PathSegmentType::Bool).unwrap();

        assert_eq!(value, SchemaValue::Bool(true));
    }

    #[test]
    fn parse_uuid_success_and_rejects_invalid_input() {
        let text = "dd00721b-3329-4621-a01d-c71f02cd78c6";
        let value = parse_path_segment_value(text.to_string(), &PathSegmentType::Uuid).unwrap();

        assert_eq!(value, SchemaValue::Uuid(text.parse().unwrap()));
        assert!(
            parse_path_segment_value("not-a-uuid".to_string(), &PathSegmentType::Uuid).is_err()
        );
    }

    #[test]
    fn parse_enum_success() {
        let enum_type = PathSegmentType::Enum(vec!["red".into(), "green".into(), "blue".into()]);

        let value = parse_path_segment_value("green".to_string(), &enum_type).unwrap();

        assert_eq!(value, SchemaValue::Enum { case: 1 });
    }

    #[test]
    fn parse_enum_failure() {
        let enum_type = PathSegmentType::Enum(vec!["a".into(), "b".into()]);

        let err = parse_path_segment_value("c".to_string(), &enum_type).unwrap_err();

        assert!(let             HttpParameterError::ValueParsingFailed {
            expected: "enum variant",
            ..
        } = err);
    }
}

#[cfg(test)]
mod query_or_header_tests {
    use assert2::assert;
    use golem_common::schema::SchemaValue;
    use golem_service_base::custom_api::PathSegmentType;
    use golem_service_base::custom_api::QueryOrHeaderType;
    use golem_service_base::custom_api::parameter_parsing::{
        HttpParameterError, parse_query_or_header_value, parse_query_parameter,
    };
    use std::collections::HashMap;
    use test_r::test;

    #[test]
    fn query_collections_preserve_values_and_require_presence() {
        let list = QueryOrHeaderType::List {
            name: None,
            owner: None,
            inner: Box::new(PathSegmentType::U8),
        };
        let map = QueryOrHeaderType::Map {
            key: PathSegmentType::U32,
            value: PathSegmentType::Bool,
        };
        let request = super::super::RichRequest::new(
            poem::Request::builder()
                .uri(
                    "/?tags=3&tags=7&labels%5B42%5D=true&labels%5B9%5D=false"
                        .parse()
                        .unwrap(),
                )
                .finish(),
        );
        assert_eq!(
            parse_query_parameter("tags", request.query_params(), &list).unwrap(),
            SchemaValue::List {
                elements: vec![SchemaValue::U8(3), SchemaValue::U8(7)],
            }
        );
        let SchemaValue::Map { entries } =
            parse_query_parameter("labels", request.query_params(), &map).unwrap()
        else {
            panic!("not a map")
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&(SchemaValue::U32(42), SchemaValue::Bool(true))));
        assert!(entries.contains(&(SchemaValue::U32(9), SchemaValue::Bool(false))));
        for ty in [list, map.clone()] {
            assert!(parse_query_parameter("missing", request.query_params(), &ty).is_err());
            let optional = QueryOrHeaderType::Option {
                name: None,
                owner: None,
                inner: Box::new(ty),
            };
            assert_eq!(
                parse_query_parameter("missing", request.query_params(), &optional).unwrap(),
                SchemaValue::Option { inner: None }
            );
        }
        let optional = QueryOrHeaderType::Option {
            name: None,
            owner: None,
            inner: Box::new(map),
        };
        assert!(matches!(
            parse_query_parameter("labels", request.query_params(), &optional).unwrap(),
            SchemaValue::Option { inner: Some(_) }
        ));
    }

    #[test]
    fn identical_query_maps_produce_identical_invocation_values() {
        let ty = QueryOrHeaderType::Map {
            key: PathSegmentType::Str,
            value: PathSegmentType::Bool,
        };
        let mut expected = None;
        for rotation in 0..32 {
            let mut query = HashMap::new();
            let entries = [
                ("labels[a]", "true"),
                ("labels[b]", "false"),
                ("labels[c]", "true"),
            ];
            for index in 0..entries.len() {
                let (key, value) = entries[(rotation + index) % entries.len()];
                query.insert(key.to_string(), vec![value.to_string()]);
            }
            let value = parse_query_parameter("labels", &query, &ty).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(
                    &value, expected,
                    "map must not depend on request hash or insertion order"
                );
            } else {
                expected = Some(value);
            }
        }
    }

    #[test]
    fn query_map_rejects_repeated_and_equivalent_keys_and_scalar_form() {
        let map = QueryOrHeaderType::Map {
            key: PathSegmentType::U32,
            value: PathSegmentType::Bool,
        };
        for query in [
            "labels[1]=true&labels[1]=false",
            "labels[1]=true&labels[01]=false",
            "labels=true",
            "labels[2]=invalid",
        ] {
            let request = super::super::RichRequest::new(
                poem::Request::builder()
                    .uri(format!("/?{query}").parse().unwrap())
                    .finish(),
            );
            assert!(
                parse_query_parameter("labels", request.query_params(), &map).is_err(),
                "{query}"
            );
        }
    }

    #[test]
    fn primitive_single_value_ok() {
        let values = vec!["42".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap();

        assert_eq!(result, SchemaValue::U32(42));
    }

    #[test]
    fn primitive_missing_value() {
        let values: Vec<String> = vec![];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap_err();

        assert!(let HttpParameterError::MissingValue { .. } = err);
    }

    #[test]
    fn primitive_too_many_values() {
        let values = vec!["1".to_string(), "2".to_string()];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap_err();

        assert!(let HttpParameterError::TooManyValues { .. } = err);
    }

    #[test]
    fn option_no_value() {
        let values: Vec<String> = vec![];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(QueryOrHeaderType::Primitive(PathSegmentType::Bool)),
            },
        )
        .unwrap();

        assert_eq!(result, SchemaValue::Option { inner: None });
    }

    #[test]
    fn option_single_value() {
        let values = vec!["true".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(QueryOrHeaderType::Primitive(PathSegmentType::Bool)),
            },
        )
        .unwrap();

        assert_eq!(
            result,
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Bool(true)))
            }
        );
    }

    #[test]
    fn option_too_many_values() {
        let values = vec!["true".to_string(), "false".to_string()];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(QueryOrHeaderType::Primitive(PathSegmentType::Bool)),
            },
        )
        .unwrap_err();

        assert!(let HttpParameterError::TooManyValues { .. } = err);
    }

    #[test]
    fn repeated_options_share_one_presence_decision() {
        let ty = QueryOrHeaderType::Option {
            name: None,
            owner: None,
            inner: Box::new(QueryOrHeaderType::Option {
                name: None,
                owner: None,
                inner: Box::new(QueryOrHeaderType::Primitive(PathSegmentType::U32)),
            }),
        };
        assert_eq!(
            parse_query_or_header_value(&[], &ty).unwrap(),
            SchemaValue::Option { inner: None }
        );
        let expected = SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::U32(3))),
            })),
        };
        assert_eq!(
            parse_query_or_header_value(&["3".into()], &ty).unwrap(),
            expected
        );
        let request = super::super::RichRequest::new(
            poem::Request::builder()
                .uri("/?limit=3".parse().unwrap())
                .finish(),
        );
        assert_eq!(
            parse_query_parameter("limit", request.query_params(), &ty).unwrap(),
            expected
        );
        assert_eq!(
            parse_query_parameter("missing", request.query_params(), &ty).unwrap(),
            SchemaValue::Option { inner: None }
        );
        assert!(parse_query_or_header_value(&[String::new()], &ty).is_err());
    }

    #[test]
    fn list_values() {
        let values = vec!["1".to_string(), "2".to_string(), "3".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::List {
                owner: None,
                name: None,
                inner: Box::new(PathSegmentType::U8),
            },
        )
        .unwrap();

        assert_eq!(
            result,
            SchemaValue::List {
                elements: vec![SchemaValue::U8(1), SchemaValue::U8(2), SchemaValue::U8(3),]
            }
        );
        let optional_list = QueryOrHeaderType::Option {
            name: None,
            owner: None,
            inner: Box::new(QueryOrHeaderType::List {
                name: None,
                owner: None,
                inner: Box::new(PathSegmentType::U8),
            }),
        };
        assert_eq!(
            parse_query_or_header_value(&values, &optional_list).unwrap(),
            SchemaValue::Option {
                inner: Some(Box::new(result)),
            }
        );
        assert_eq!(
            parse_query_or_header_value(&[], &optional_list).unwrap(),
            SchemaValue::Option { inner: None }
        );
    }
}

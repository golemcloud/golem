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

use test_r::test;

use super::*;

mod sub_parsers {
    use test_r::test;

    use crate::model::lucene::{Query, clause, disjunct_query, term};
    use combine::{EasyParser, attempt, optional};

    #[test]
    fn term1() {
        let query = "hello".to_string();
        let (parsed, _) = term().easy_parse(query.as_str()).unwrap();
        assert_eq!(parsed, "hello");
    }

    #[test]
    fn term2() {
        let query = "hello world".to_string();
        let (parsed, _) = term().easy_parse(query.as_str()).unwrap();
        assert_eq!(parsed, "hello");
    }

    #[test]
    fn term3() {
        let query = "hello:".to_string();
        let (parsed, _) = term().easy_parse(query.as_str()).unwrap();
        assert_eq!(parsed, "hello");
    }

    #[test]
    fn term4() {
        let query = "he\\llo".to_string();
        let (parsed, _) = term().easy_parse(query.as_str()).unwrap();
        assert_eq!(parsed, "hello");
    }

    #[test]
    fn term_keyword() {
        let query = "AND".to_string();
        let result = term().easy_parse(query.as_str());
        assert!(result.is_err());
    }

    #[test]
    fn regex_term_query() {
        let query = "/$hello([a-z]+)?/";
        let (parsed, _) = super::regexp_term_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Regex {
                pattern: "$hello([a-z]+)?".to_string()
            }
        );
    }

    #[test]
    fn quoted_term_query1() {
        let query = "\"hello world\"";
        let (parsed, _) = super::quoted_term_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Phrase {
                value: "hello world".to_string()
            }
        );
    }

    #[test]
    fn quoted_term_query2() {
        let query = "\"hello world\" again";
        let (parsed, _) = super::quoted_term_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Phrase {
                value: "hello world".to_string()
            }
        );
    }

    #[test]
    fn field_term_query() {
        let query = "hello";
        let (parsed, _) = super::field_term_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Term {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn field_name_prefix_1() {
        let query = "field:hello";
        let (parsed, _) = optional(attempt(super::field_name_prefix()))
            .easy_parse(query)
            .unwrap();
        assert_eq!(parsed, Some("field".to_string()));
    }

    #[test]
    fn field_name_prefix_2() {
        let query = "hello";
        let mut p = optional(attempt(super::field_name_prefix()));
        let (parsed, _) = p.easy_parse(query).unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn clause_1() {
        let query = "hello";
        let (parsed, _) = clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Term {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn clause_2() {
        let query = "field:hello";
        let (parsed, _) = clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Field {
                field: "field".to_string(),
                query: Box::new(Query::Term {
                    value: "hello".to_string()
                })
            }
        );
    }

    #[test]
    fn clause_3() {
        let query = "\"hello world\"";
        let (parsed, _) = clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Phrase {
                value: "hello world".to_string()
            }
        );
    }

    #[test]
    fn clause_4() {
        let query = "/regex/";
        let (parsed, _) = clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Regex {
                pattern: "regex".to_string()
            }
        );
    }

    #[test]
    fn mod_clause_1() {
        let query = "hello";
        let (parsed, _) = super::mod_clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Term {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn mod_clause_2() {
        let query = "NOT hello";
        let (parsed, _) = super::mod_clause().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Not {
                query: Box::new(Query::Term {
                    value: "hello".to_string()
                })
            }
        );
    }

    #[test]
    fn conjunct_query_1() {
        let query = "hello";
        let (parsed, _) = super::conjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Term {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn conjunct_query_2() {
        let query = "hello AND world";
        let (parsed, _) = super::conjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::And {
                queries: vec![
                    Query::Term {
                        value: "hello".to_string()
                    },
                    Query::Term {
                        value: "world".to_string()
                    }
                ]
            }
        );
    }

    #[test]
    fn conjunct_query_3() {
        let query = "hello AND NOT world";
        let (parsed, _) = super::conjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::And {
                queries: vec![
                    Query::Term {
                        value: "hello".to_string()
                    },
                    Query::Not {
                        query: Box::new(Query::Term {
                            value: "world".to_string()
                        })
                    }
                ]
            }
        );
    }

    #[test]
    fn disjunct_query_1() {
        let query = "hello";
        let (parsed, _) = super::disjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Term {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn disjunct_query_2() {
        let query = "hello OR world";
        let (parsed, _) = super::disjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Or {
                queries: vec![
                    Query::Term {
                        value: "hello".to_string()
                    },
                    Query::Term {
                        value: "world".to_string()
                    }
                ]
            }
        );
    }

    #[test]
    fn disjunct_query_3() {
        let query = "hello OR NOT world AND NOT vilag";
        let (parsed, _) = disjunct_query().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::Or {
                queries: vec![
                    Query::Term {
                        value: "hello".to_string()
                    },
                    Query::And {
                        queries: vec![
                            Query::Not {
                                query: Box::new(Query::Term {
                                    value: "world".to_string()
                                })
                            },
                            Query::Not {
                                query: Box::new(Query::Term {
                                    value: "vilag".to_string()
                                })
                            }
                        ]
                    }
                ]
            }
        );
    }

    #[test]
    fn grouping() {
        let query = "(hello world)";
        let (parsed, _) = super::grouping().easy_parse(query).unwrap();
        assert_eq!(
            parsed,
            Query::And {
                queries: vec![
                    Query::Term {
                        value: "hello".to_string()
                    },
                    Query::Term {
                        value: "world".to_string()
                    }
                ]
            }
        )
    }

    #[test]
    fn grouping_mismatch() {
        let query = "hello world";
        let result = super::grouping().easy_parse(query);
        assert!(result.is_err())
    }
}

#[test]
fn simple_term() -> Result<(), String> {
    let query = "hello";
    let parsed = Query::parse(query)?;

    assert_eq!(
        parsed,
        Query::Term {
            value: "hello".to_string()
        }
    );
    Ok(())
}

#[test]
fn simple_field() -> Result<(), String> {
    let query = "field:hello";
    let parsed = Query::parse(query)?;

    assert_eq!(
        parsed,
        Query::Field {
            field: "field".to_string(),
            query: Box::new(Query::Term {
                value: "hello".to_string()
            })
        }
    );
    Ok(())
}

#[test]
fn example1() -> Result<(), String> {
    let query = "hello AND (world OR vilag) NOT other";
    let parsed = Query::parse(query)?;

    assert_eq!(
        parsed,
        Query::And {
            queries: vec![
                Query::And {
                    queries: vec![
                        Query::Term {
                            value: "hello".to_string()
                        },
                        Query::Or {
                            queries: vec![
                                Query::Term {
                                    value: "world".to_string()
                                },
                                Query::Term {
                                    value: "vilag".to_string()
                                }
                            ]
                        }
                    ]
                },
                Query::Not {
                    query: Box::new(Query::Term {
                        value: "other".to_string()
                    })
                }
            ]
        }
    );
    Ok(())
}

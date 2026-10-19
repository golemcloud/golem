use golem_schema::schema::canonical::{binary, quantity};
use golem_schema::schema::render::{from_json_value, to_json_value};
use golem_schema::schema::tool::validation::is_valid_identifier;
use golem_schema::schema::validation::{validate_graph, validate_value};
use golem_schema::schema::*;
use test_r::test;

test_r::enable!();

#[test]
fn plain_values_and_constraints_work_without_rich_features() {
    let graph = SchemaGraph::anonymous(SchemaType::Text {
        restrictions: TextRestrictions {
            min_length: Some(2),
            max_length: Some(3),
            ..Default::default()
        },
        metadata: Default::default(),
    });
    validate_graph(&graph).unwrap();
    for (text, valid) in [
        ("é", false),
        ("é中", true),
        ("é中x", true),
        ("é中xy", false),
    ] {
        let value = SchemaValue::Text(TextValuePayload {
            text: text.into(),
            language: None,
        });
        assert_eq!(validate_value(&graph, &graph.root, &value).is_ok(), valid);
    }
}

#[test]
fn regex_validation_is_opt_in_not_a_smaller_dialect() {
    let graph = SchemaGraph::anonymous(SchemaType::Text {
        restrictions: TextRestrictions {
            regex: Some(r"\p{Greek}+".into()),
            ..Default::default()
        },
        metadata: Default::default(),
    });
    assert_eq!(validate_graph(&graph).is_ok(), cfg!(feature = "regex"));
    for (text, matches) in [("prefix αβ suffix", true), ("abc", false)] {
        let value = SchemaValue::Text(TextValuePayload {
            text: text.into(),
            language: None,
        });
        let result = validate_value(&graph, &graph.root, &value);
        assert_eq!(result.is_ok(), cfg!(feature = "regex") && matches);
        #[cfg(not(feature = "regex"))]
        assert!(matches!(
            &result.unwrap_err()[0],
            validation::ValueError::UnsupportedFeature {
                feature: "regex",
                ..
            }
        ));
    }
}

#[test]
fn invalid_regex_is_rejected_by_schema_validation() {
    let graph = SchemaGraph::anonymous(SchemaType::Text {
        restrictions: TextRestrictions {
            regex: Some("[".into()),
            ..Default::default()
        },
        metadata: Default::default(),
    });
    assert!(validate_graph(&graph).is_err());
}

#[test]
fn url_validation_preserves_idna_and_scheme_contract() {
    let graph = SchemaGraph::anonymous(SchemaType::Url {
        restrictions: UrlRestrictions {
            allowed_schemes: Some(vec!["HTTPS".into()]),
            allowed_hosts: Some(vec!["xn--bcher-kva.example".into()]),
        },
        metadata: Default::default(),
    });
    validate_graph(&graph).unwrap();
    for (url, valid) in [
        ("https://bücher.example/path", true),
        ("http://bücher.example/path", false),
        ("https://other.example/path", false),
        ("not a URL", false),
    ] {
        let value = SchemaValue::Url { url: url.into() };
        let result = validate_value(&graph, &graph.root, &value);
        assert_eq!(result.is_ok(), cfg!(feature = "url") && valid);
        #[cfg(not(feature = "url"))]
        assert!(matches!(
            &result.unwrap_err()[0],
            validation::ValueError::UnsupportedFeature { feature: "url", .. }
        ));
    }
}

#[test]
fn regex_union_cannot_fall_through_when_disabled() {
    let mut graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "greek".into(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Regex {
                regex: r"^\p{Greek}+$".into(),
            },
            metadata: Default::default(),
        }],
    }));
    let value = SchemaValue::Union(UnionValuePayload {
        tag: "greek".into(),
        body: Box::new(SchemaValue::String("αβ".into())),
    });
    assert_eq!(validate_graph(&graph).is_ok(), cfg!(feature = "regex"));
    assert_eq!(
        validate_value(&graph, &graph.root, &value).is_ok(),
        cfg!(feature = "regex")
    );
    let json = serde_json::json!("αβ");
    assert_eq!(
        to_json_value(&graph, &graph.root, &value).is_ok(),
        cfg!(feature = "regex")
    );
    assert_eq!(
        from_json_value(&graph, &graph.root, &json).is_ok(),
        cfg!(feature = "regex")
    );
    assert!(from_json_value(&graph, &graph.root, &serde_json::json!("abc")).is_err());

    // An unavailable regex must not be treated as a non-matching branch when
    // another discriminator matches, which could select the wrong branch.
    let SchemaType::Union { spec, .. } = &mut graph.root else {
        unreachable!()
    };
    spec.branches.push(UnionBranch {
        tag: "latin".into(),
        body: SchemaType::string(),
        discriminator: DiscriminatorRule::Prefix {
            prefix: "abc".into(),
        },
        metadata: Default::default(),
    });
    let decoded = from_json_value(&graph, &graph.root, &serde_json::json!("abcdef"));
    #[cfg(not(feature = "regex"))]
    assert!(matches!(decoded, Err(render::RenderError::Unsupported(_))));
    #[cfg(feature = "regex")]
    assert_eq!(
        decoded.unwrap(),
        SchemaValue::Union(UnionValuePayload {
            tag: "latin".into(),
            body: Box::new(SchemaValue::String("abcdef".into())),
        })
    );
}

#[test]
fn fixed_grammars_keep_their_character_sets() {
    for (mime, valid) in [
        ("A0/x!#$&^_.+-", true),
        ("a/b/c", false),
        ("a/", false),
        ("/b", false),
        ("a/b\n", false),
        ("é/b", false),
        ("a/b;c=d", false),
    ] {
        assert_eq!(
            binary::to_text(&BinaryValuePayload {
                bytes: vec![1],
                mime_type: Some(mime.into())
            })
            .is_ok(),
            valid,
            "{mime:?}"
        );
    }
    for (unit, valid) in [
        ("", true),
        ("AZ09%°µμ_/-^", true),
        ("Ω", false),
        ("²", false),
        ("kg\n", false),
        ("kg m", false),
    ] {
        assert_eq!(
            quantity::to_text(&QuantityValue {
                mantissa: 123,
                scale: 2,
                unit: unit.into()
            })
            .is_ok(),
            valid,
            "{unit:?}"
        );
    }
    for (name, valid) in [
        ("a", true),
        ("a0-9b", true),
        ("0a", false),
        ("a--b", false),
        ("a-", false),
        ("a\n", false),
        ("a-é", false),
    ] {
        assert_eq!(is_valid_identifier(name), valid, "{name:?}");
    }
}

#[cfg(feature = "regex")]
mod differential {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    proptest! {
        #[test]
        fn fixed_grammars_match_original_regexes(
            token in "[A-Za-z0-9!#$&^_.+\\-]{1,12}",
            unit in "[A-Za-z0-9%°µμ_/\\-^]{0,12}",
            name in "[a-z][a-z0-9]{0,6}(-[a-z0-9]{1,6}){0,3}",
            inserted in any::<char>(),
        ) {
            let mime_re = regex::Regex::new(r"^[A-Za-z0-9!#$&^_.+\-]+/[A-Za-z0-9!#$&^_.+\-]+$").unwrap();
            let unit_re = regex::Regex::new(r"^[A-Za-z0-9%°µμ_/\-^]+$").unwrap();
            let name_re = regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap();
            for mime in [format!("{token}/x"), format!("{token}{inserted}/x"), format!("x/{inserted}{token}")] {
                prop_assert_eq!(binary::to_text(&BinaryValuePayload { bytes: vec![], mime_type: Some(mime.clone()) }).is_ok(), mime_re.is_match(&mime));
            }
            for unit in [unit.clone(), format!("{unit}{inserted}")] {
                prop_assert_eq!(quantity::to_text(&QuantityValue { mantissa: 1, scale: 0, unit: unit.clone() }).is_ok(), unit.is_empty() || unit_re.is_match(&unit));
            }
            for name in [name.clone(), format!("{name}{inserted}"), format!("{inserted}{name}")] {
                prop_assert_eq!(is_valid_identifier(&name), name_re.is_match(&name));
            }
        }
    }
}

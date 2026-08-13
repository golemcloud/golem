// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
use golem_common::schema::metadata::{MetadataEnvelope, Role, TypeId};
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, NumericBound,
    NumericRestrictions, PathDirection, PathKind, PathSpec, QuantitySpec, QuantityValue,
    QuotaTokenSpec, ResultSpec, SchemaType, SecretSpec, TextRestrictions, UnionBranch, UnionSpec,
    UrlRestrictions, VariantCaseType,
};

pub fn realistic_schema_graph() -> SchemaGraph {
    let node_id = TypeId::new("fixture.Node");
    let error_id = TypeId::new("fixture.Error");
    SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: node_id.clone(),
                name: Some("fixture.Node".into()),
                body: SchemaType::record(vec![
                    field("label", SchemaType::string()),
                    field(
                        "next",
                        SchemaType::option(SchemaType::ref_to(node_id.clone())),
                    ),
                ]),
            },
            SchemaTypeDef {
                id: error_id.clone(),
                name: Some("fixture.Error".into()),
                body: SchemaType::variant(vec![
                    variant_case("invalid", Some(SchemaType::string())),
                    variant_case("unavailable", None),
                ]),
            },
        ],
        root: SchemaType::record(vec![
            field("local-config", SchemaType::ref_to(node_id.clone())),
            field(
                "tool-input",
                SchemaType::list(SchemaType::ref_to(node_id.clone())),
            ),
            field("tool-result", SchemaType::ref_to(node_id)),
            field("declared-error", SchemaType::ref_to(error_id)),
        ]),
    }
}

pub fn exhaustive_schema_graph() -> SchemaGraph {
    let escaped = "quote \" slash \\ newline\n tab\t emoji 😀";
    let root_metadata = metadata(Role::Other(escaped.into()));
    let recursive_id = TypeId::new("fixture.Recursive");
    let recursive = SchemaTypeDef {
        id: recursive_id.clone(),
        name: Some(escaped.into()),
        body: SchemaType::record(vec![
            field("value", SchemaType::string()),
            field(
                "next",
                SchemaType::option(SchemaType::ref_to(recursive_id.clone())),
            ),
        ]),
    };

    let mut multimodal_field = field("bool", SchemaType::bool());
    multimodal_field.metadata = metadata(Role::Multimodal);
    let mut text_case = variant_case("text", Some(SchemaType::string()));
    text_case.metadata = metadata(Role::UnstructuredText);

    let mut union_branches = vec![
        union_branch(
            "prefix",
            SchemaType::string(),
            DiscriminatorRule::Prefix {
                prefix: "pre-".into(),
            },
        ),
        union_branch(
            "suffix",
            SchemaType::string(),
            DiscriminatorRule::Suffix {
                suffix: "-suffix".into(),
            },
        ),
        union_branch(
            "contains",
            SchemaType::string(),
            DiscriminatorRule::Contains {
                substring: "middle".into(),
            },
        ),
        union_branch(
            "regex",
            SchemaType::string(),
            DiscriminatorRule::Regex {
                regex: "^regex:[0-9]+$".into(),
            },
        ),
        union_branch(
            "field-equals",
            SchemaType::record(vec![field("kind", SchemaType::string())]),
            DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "kind".into(),
                literal: Some(escaped.into()),
            }),
        ),
        union_branch(
            "field-absent",
            SchemaType::record(vec![field("other", SchemaType::string())]),
            DiscriminatorRule::FieldAbsent {
                field_name: "kind".into(),
            },
        ),
    ];
    union_branches[0].metadata = metadata(Role::UnstructuredBinary);

    SchemaGraph {
        defs: vec![recursive],
        root: SchemaType::record(vec![
            field("recursive-a", SchemaType::ref_to(recursive_id.clone())),
            field("recursive-b", SchemaType::ref_to(recursive_id)),
            multimodal_field,
            field("s8", SchemaType::s8()),
            field("s16", SchemaType::s16()),
            field("s32", SchemaType::s32()),
            field(
                "s64",
                numeric_type(
                    true,
                    NumericBound::Signed(i64::MIN),
                    NumericBound::Signed(i64::MAX),
                ),
            ),
            field("u8", SchemaType::u8()),
            field("u16", SchemaType::u16()),
            field("u32", SchemaType::u32()),
            field(
                "u64",
                numeric_type(
                    false,
                    NumericBound::Unsigned(0),
                    NumericBound::Unsigned(u64::MAX),
                ),
            ),
            field(
                "u64-high-bit",
                numeric_type(
                    false,
                    NumericBound::Unsigned(1 << 63),
                    NumericBound::Unsigned(1 << 63),
                ),
            ),
            field("f32", SchemaType::f32()),
            field(
                "f64",
                SchemaType::F64 {
                    restrictions: Some(NumericRestrictions {
                        min: Some(NumericBound::FloatBits((-2.0f64).to_bits())),
                        max: Some(NumericBound::FloatBits((-1.0f64).to_bits())),
                        unit: Some("ratio".into()),
                    }),
                    metadata: MetadataEnvelope::default(),
                },
            ),
            field("char", SchemaType::char()),
            field("string", SchemaType::string()),
            field(
                "record",
                SchemaType::record(vec![field(escaped, SchemaType::string())]),
            ),
            field(
                "variant",
                SchemaType::variant(vec![text_case, variant_case("none", None)]),
            ),
            field(
                "enum",
                SchemaType::r#enum(vec![escaped.into(), "plain".into()]),
            ),
            field(
                "flags",
                SchemaType::flags(vec![escaped.into(), "plain".into()]),
            ),
            field(
                "tuple",
                SchemaType::tuple(vec![SchemaType::string(), SchemaType::u64()]),
            ),
            field("list", SchemaType::list(SchemaType::string())),
            field(
                "fixed-list",
                SchemaType::fixed_list(SchemaType::u8(), u32::MAX),
            ),
            field(
                "fixed-list-high-bit",
                SchemaType::fixed_list(SchemaType::u8(), 1 << 31),
            ),
            field(
                "map",
                SchemaType::map(SchemaType::string(), SchemaType::u64()),
            ),
            field("option", SchemaType::option(SchemaType::string())),
            field(
                "result",
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::string())),
                    err: Some(Box::new(SchemaType::u32())),
                }),
            ),
            field(
                "text",
                SchemaType::text(TextRestrictions {
                    languages: Some(vec!["en".into(), escaped.into()]),
                    min_length: Some(0),
                    max_length: Some(u32::MAX),
                    regex: Some("^[^\n]+$".into()),
                }),
            ),
            field(
                "binary",
                SchemaType::binary(BinaryRestrictions {
                    mime_types: Some(vec!["application/octet-stream".into()]),
                    min_bytes: Some(0),
                    max_bytes: Some(u32::MAX),
                }),
            ),
            field(
                "path",
                SchemaType::path(PathSpec {
                    direction: PathDirection::InOut,
                    kind: PathKind::Any,
                    allowed_mime_types: Some(vec!["text/plain".into()]),
                    allowed_extensions: Some(vec![escaped.into()]),
                }),
            ),
            field(
                "url",
                SchemaType::url(UrlRestrictions {
                    allowed_schemes: Some(vec!["https".into()]),
                    allowed_hosts: Some(vec![escaped.into()]),
                }),
            ),
            field("datetime", SchemaType::datetime()),
            field("duration", SchemaType::duration()),
            field(
                "quantity",
                SchemaType::quantity(QuantitySpec {
                    base_unit: escaped.into(),
                    allowed_suffixes: vec!["min".into(), "max".into()],
                    min: Some(QuantityValue {
                        mantissa: i64::MIN,
                        scale: i32::MIN,
                        unit: "min".into(),
                    }),
                    max: Some(QuantityValue {
                        mantissa: i64::MAX,
                        scale: i32::MAX,
                        unit: "max".into(),
                    }),
                }),
            ),
            field(
                "union",
                SchemaType::union(UnionSpec {
                    branches: union_branches,
                }),
            ),
            field(
                "secret",
                SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::string()),
                    category: Some(escaped.into()),
                }),
            ),
            field(
                "quota-token",
                SchemaType::quota_token(QuotaTokenSpec {
                    resource_name: Some(escaped.into()),
                }),
            ),
            field("future", SchemaType::future(Some(SchemaType::string()))),
            field("stream", SchemaType::stream(None)),
        ])
        .with_metadata(root_metadata),
    }
}

fn numeric_type(signed: bool, min: NumericBound, max: NumericBound) -> SchemaType {
    let restrictions = Some(NumericRestrictions {
        min: Some(min),
        max: Some(max),
        unit: Some("edge".into()),
    });
    if signed {
        SchemaType::S64 {
            restrictions,
            metadata: MetadataEnvelope::default(),
        }
    } else {
        SchemaType::U64 {
            restrictions,
            metadata: MetadataEnvelope::default(),
        }
    }
}

fn metadata(role: Role) -> MetadataEnvelope {
    MetadataEnvelope {
        doc: Some("docs \"quoted\"\nnext".into()),
        aliases: vec!["alias\\one".into()],
        examples: vec!["{\"value\":1}".into()],
        deprecated: Some("deprecated\tmessage".into()),
        role: Some(role),
    }
}

fn field(name: impl Into<String>, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.into(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

fn variant_case(name: impl Into<String>, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.into(),
        payload,
        metadata: MetadataEnvelope::default(),
    }
}

fn union_branch(
    tag: impl Into<String>,
    body: SchemaType,
    discriminator: DiscriminatorRule,
) -> UnionBranch {
    UnionBranch {
        tag: tag.into(),
        body,
        discriminator,
        metadata: MetadataEnvelope::default(),
    }
}

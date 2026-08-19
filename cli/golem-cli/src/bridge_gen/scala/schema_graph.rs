// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::scala_string_literal;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::metadata::{MetadataEnvelope, Role};
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NumericBound, NumericRestrictions, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, SchemaType, TextRestrictions,
    UrlRestrictions,
};

const SCHEMA: &str = "_root_.golem.schema";
const LIST: &str = "_root_.scala.collection.immutable.List";
const SOME: &str = "_root_.scala.Some";
const NONE: &str = "_root_.scala.None";

pub(crate) const REGISTRY_OBJECT: &str = "__GolemSchemaGraphs";

#[derive(Default)]
pub(crate) struct SchemaGraphRegistry {
    graphs: Vec<SchemaGraph>,
}

impl SchemaGraphRegistry {
    pub(crate) fn intern(&mut self, graph: SchemaGraph) -> String {
        let index = self
            .graphs
            .iter()
            .position(|known| known == &graph)
            .unwrap_or_else(|| {
                let index = self.graphs.len();
                self.graphs.push(graph);
                index
            });
        format!("{REGISTRY_OBJECT}.graph{index}")
    }

    pub(crate) fn definition(&self) -> Option<String> {
        (!self.graphs.is_empty()).then(|| {
            let graphs = self
                .graphs
                .iter()
                .enumerate()
                .map(|(index, graph)| {
                    format!(
                        "  lazy val graph{index}: {SCHEMA}.SchemaGraph = {}",
                        emit_schema_graph_literal(graph)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("private object {REGISTRY_OBJECT} {{\n{graphs}\n}}")
        })
    }
}

pub(crate) fn emit_schema_graph_literal(graph: &SchemaGraph) -> String {
    let defs = graph
        .defs
        .iter()
        .map(|def| {
            format!(
                "({}, {SCHEMA}.SchemaTypeDef({}, {}))",
                string(def.id.as_str()),
                emit_schema_type(&def.body),
                option_string(def.name.as_deref())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{SCHEMA}.SchemaGraph(_root_.scala.collection.immutable.ListMap[_root_.scala.Predef.String, {SCHEMA}.SchemaTypeDef]({defs}), {})",
        emit_schema_type(&graph.root)
    )
}

fn emit_schema_type(typ: &SchemaType) -> String {
    use SchemaType::*;

    let body = match typ {
        Ref { id, .. } => format!("{SCHEMA}.SchemaTypeBody.RefType({})", string(id.as_str())),
        Bool { .. } => format!("{SCHEMA}.SchemaTypeBody.BoolType"),
        S8 { restrictions, .. } => numeric("S8", restrictions.as_ref()),
        S16 { restrictions, .. } => numeric("S16", restrictions.as_ref()),
        S32 { restrictions, .. } => numeric("S32", restrictions.as_ref()),
        S64 { restrictions, .. } => numeric("S64", restrictions.as_ref()),
        U8 { restrictions, .. } => numeric("U8", restrictions.as_ref()),
        U16 { restrictions, .. } => numeric("U16", restrictions.as_ref()),
        U32 { restrictions, .. } => numeric("U32", restrictions.as_ref()),
        U64 { restrictions, .. } => numeric("U64", restrictions.as_ref()),
        F32 { restrictions, .. } => numeric("F32", restrictions.as_ref()),
        F64 { restrictions, .. } => numeric("F64", restrictions.as_ref()),
        Char { .. } => format!("{SCHEMA}.SchemaTypeBody.CharType"),
        String { .. } => format!("{SCHEMA}.SchemaTypeBody.StringType"),
        Record { fields, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.RecordType({LIST}({}))",
            fields
                .iter()
                .map(|field| format!(
                    "{SCHEMA}.NamedFieldType({}, {}, {})",
                    string(&field.name),
                    emit_schema_type(&field.body),
                    metadata(&field.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Variant { cases, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.VariantType({LIST}({}))",
            cases
                .iter()
                .map(|case| format!(
                    "{SCHEMA}.VariantCaseType({}, {}, {})",
                    string(&case.name),
                    option_type(case.payload.as_ref()),
                    metadata(&case.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Enum { cases, .. } => format!("{SCHEMA}.SchemaTypeBody.EnumType({})", strings(cases)),
        Flags { flags, .. } => {
            format!("{SCHEMA}.SchemaTypeBody.FlagsType({})", strings(flags))
        }
        Tuple { elements, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.TupleType({LIST}({}))",
            elements
                .iter()
                .map(emit_schema_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        List { element, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.ListType({})",
            emit_schema_type(element)
        ),
        FixedList {
            element, length, ..
        } => format!(
            "{SCHEMA}.SchemaTypeBody.FixedListType({}, {})",
            emit_schema_type(element),
            raw_int(*length)
        ),
        Map { key, value, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.MapType({}, {})",
            emit_schema_type(key),
            emit_schema_type(value)
        ),
        Option { inner, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.OptionType({})",
            emit_schema_type(inner)
        ),
        Result { spec, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.ResultType({}, {})",
            option_type(spec.ok.as_deref()),
            option_type(spec.err.as_deref())
        ),
        Text { restrictions, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.TextType({})",
            text_restrictions(restrictions)
        ),
        Binary { restrictions, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.BinaryType({})",
            binary_restrictions(restrictions)
        ),
        Path { spec, .. } => {
            format!("{SCHEMA}.SchemaTypeBody.PathType({})", path_spec(spec))
        }
        Url { restrictions, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.UrlType({})",
            url_restrictions(restrictions)
        ),
        Datetime { .. } => format!("{SCHEMA}.SchemaTypeBody.DatetimeType"),
        Duration { .. } => format!("{SCHEMA}.SchemaTypeBody.DurationType"),
        Quantity { spec, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.QuantityType({})",
            quantity_spec(spec)
        ),
        Union { spec, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.UnionType({LIST}({}))",
            spec.branches
                .iter()
                .map(|branch| format!(
                    "{SCHEMA}.UnionBranch({}, {}, {}, {})",
                    string(&branch.tag),
                    emit_schema_type(&branch.body),
                    discriminator(&branch.discriminator),
                    metadata(&branch.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Secret { spec, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.SecretType({SCHEMA}.SecretSpec({}, {}))",
            emit_schema_type(&spec.inner),
            option_string(spec.category.as_deref())
        ),
        QuotaToken { spec, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.QuotaTokenType({})",
            quota_token_spec(spec)
        ),
        Future { inner, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.FutureType({})",
            option_type(inner.as_deref())
        ),
        Stream { inner, .. } => format!(
            "{SCHEMA}.SchemaTypeBody.StreamType({})",
            option_type(inner.as_deref())
        ),
    };
    format!("{SCHEMA}.SchemaType({body}, {})", metadata(typ.metadata()))
}

fn numeric(name: &str, restrictions: Option<&NumericRestrictions>) -> String {
    format!(
        "{SCHEMA}.SchemaTypeBody.{name}Type({})",
        restrictions
            .map(|value| format!("{SOME}({})", numeric_restrictions(value)))
            .unwrap_or_else(|| NONE.to_string())
    )
}

fn numeric_restrictions(value: &NumericRestrictions) -> String {
    format!(
        "{SCHEMA}.NumericRestrictions({}, {}, {})",
        option_bound(value.min),
        option_bound(value.max),
        option_string(value.unit.as_deref())
    )
}

fn option_bound(value: Option<NumericBound>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| {
            let bound = match value {
                NumericBound::Signed(value) => {
                    format!("{SCHEMA}.NumericBound.Signed({})", signed_long(value))
                }
                NumericBound::Unsigned(value) => {
                    format!("{SCHEMA}.NumericBound.Unsigned({})", raw_long(value))
                }
                NumericBound::FloatBits(value) => {
                    format!("{SCHEMA}.NumericBound.FloatBits({})", raw_long(value))
                }
            };
            format!("{SOME}({bound})")
        },
    )
}

fn metadata(value: &MetadataEnvelope) -> String {
    if value.is_empty() {
        return format!("{SCHEMA}.MetadataEnvelope.empty");
    }
    let role = value.role.as_ref().map_or_else(
        || NONE.to_string(),
        |role| {
            let role = match role {
                Role::Multimodal => format!("{SCHEMA}.Role.Multimodal"),
                Role::UnstructuredText => format!("{SCHEMA}.Role.UnstructuredText"),
                Role::UnstructuredBinary => format!("{SCHEMA}.Role.UnstructuredBinary"),
                Role::Other(value) => format!("{SCHEMA}.Role.Other({})", string(value)),
            };
            format!("{SOME}({role})")
        },
    );
    format!(
        "{SCHEMA}.MetadataEnvelope({}, {}, {}, {}, {role})",
        option_string(value.doc.as_deref()),
        strings(&value.aliases),
        strings(&value.examples),
        option_string(value.deprecated.as_deref())
    )
}

fn text_restrictions(value: &TextRestrictions) -> String {
    format!(
        "{SCHEMA}.TextRestrictions({}, {}, {}, {})",
        option_strings(value.languages.as_deref()),
        option_raw_int(value.min_length),
        option_raw_int(value.max_length),
        option_string(value.regex.as_deref())
    )
}

fn binary_restrictions(value: &BinaryRestrictions) -> String {
    format!(
        "{SCHEMA}.BinaryRestrictions({}, {}, {})",
        option_strings(value.mime_types.as_deref()),
        option_raw_int(value.min_bytes),
        option_raw_int(value.max_bytes)
    )
}

fn path_spec(value: &PathSpec) -> String {
    let direction = match value.direction {
        PathDirection::Input => "Input",
        PathDirection::Output => "Output",
        PathDirection::InOut => "InOut",
    };
    let kind = match value.kind {
        PathKind::File => "File",
        PathKind::Directory => "Directory",
        PathKind::Any => "Any",
    };
    format!(
        "{SCHEMA}.PathSpec({SCHEMA}.PathDirection.{direction}, {SCHEMA}.PathKind.{kind}, {}, {})",
        option_strings(value.allowed_mime_types.as_deref()),
        option_strings(value.allowed_extensions.as_deref())
    )
}

fn url_restrictions(value: &UrlRestrictions) -> String {
    format!(
        "{SCHEMA}.UrlRestrictions({}, {})",
        option_strings(value.allowed_schemes.as_deref()),
        option_strings(value.allowed_hosts.as_deref())
    )
}

fn quantity_spec(value: &QuantitySpec) -> String {
    format!(
        "{SCHEMA}.QuantitySpec({}, {}, {}, {})",
        string(&value.base_unit),
        strings(&value.allowed_suffixes),
        option_quantity(value.min.as_ref()),
        option_quantity(value.max.as_ref())
    )
}

fn option_quantity(value: Option<&QuantityValue>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| {
            format!(
                "{SOME}({SCHEMA}.QuantityValue({}, {}, {}))",
                signed_long(value.mantissa),
                value.scale,
                string(&value.unit)
            )
        },
    )
}

fn discriminator(value: &DiscriminatorRule) -> String {
    match value {
        DiscriminatorRule::Prefix { prefix } => {
            format!("{SCHEMA}.DiscriminatorRule.Prefix({})", string(prefix))
        }
        DiscriminatorRule::Suffix { suffix } => {
            format!("{SCHEMA}.DiscriminatorRule.Suffix({})", string(suffix))
        }
        DiscriminatorRule::Contains { substring } => {
            format!("{SCHEMA}.DiscriminatorRule.Contains({})", string(substring))
        }
        DiscriminatorRule::Regex { regex } => {
            format!("{SCHEMA}.DiscriminatorRule.Regex({})", string(regex))
        }
        DiscriminatorRule::FieldEquals(field) => format!(
            "{SCHEMA}.DiscriminatorRule.FieldEquals({SCHEMA}.FieldDiscriminator({}, {}))",
            string(&field.field_name),
            option_string(field.literal.as_deref())
        ),
        DiscriminatorRule::FieldAbsent { field_name } => format!(
            "{SCHEMA}.DiscriminatorRule.FieldAbsent({})",
            string(field_name)
        ),
    }
}

fn quota_token_spec(value: &QuotaTokenSpec) -> String {
    format!(
        "{SCHEMA}.QuotaTokenSpec({})",
        option_string(value.resource_name.as_deref())
    )
}

fn option_type(value: Option<&SchemaType>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| format!("{SOME}({})", emit_schema_type(value)),
    )
}

fn option_string(value: Option<&str>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| format!("{SOME}({})", string(value)),
    )
}

fn option_strings(value: Option<&[String]>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| format!("{SOME}({})", strings(value)),
    )
}

fn option_raw_int(value: Option<u32>) -> String {
    value.map_or_else(
        || NONE.to_string(),
        |value| format!("{SOME}({})", raw_int(value)),
    )
}

fn strings(values: &[String]) -> String {
    format!(
        "{LIST}({})",
        values
            .iter()
            .map(|value| string(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn string(value: &str) -> String {
    scala_string_literal(value)
}

fn signed_long(value: i64) -> String {
    if value == i64::MIN {
        "_root_.scala.Long.MinValue".to_string()
    } else {
        format!("{value}L")
    }
}

fn raw_long(value: u64) -> String {
    signed_long(value as i64)
}

fn raw_int(value: u32) -> String {
    let value = value as i32;
    if value == i32::MIN {
        "_root_.scala.Int.MinValue".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_gen::schema_graph_test_fixture::{
        exhaustive_schema_graph, realistic_schema_graph,
    };
    use test_r::test;

    #[test]
    fn exhaustive_literal_is_deterministic_and_preserves_raw_integer_bits() {
        let graph = exhaustive_schema_graph();
        let source = emit_schema_graph_literal(&graph);
        let negative_two_bits = signed_long((-2.0f64).to_bits() as i64);

        assert_eq!(source, emit_schema_graph_literal(&graph));
        for expected in [
            "_root_.scala.Long.MinValue",
            "_root_.golem.schema.NumericBound.Unsigned(-1L)",
            &format!("_root_.golem.schema.NumericBound.FloatBits({negative_two_bits})"),
            "_root_.golem.schema.QuantityValue(_root_.scala.Long.MinValue, -2147483648, \"min\")",
            "_root_.golem.schema.QuantityValue(9223372036854775807L, 2147483647, \"max\")",
            "_root_.scala.Int.MinValue",
            "_root_.golem.schema.Role.Multimodal",
            "_root_.golem.schema.Role.UnstructuredText",
            "_root_.golem.schema.Role.UnstructuredBinary",
            "_root_.golem.schema.Role.Other",
            "_root_.golem.schema.DiscriminatorRule.FieldEquals",
            "_root_.golem.schema.DiscriminatorRule.FieldAbsent",
            "_root_.golem.schema.SchemaTypeBody.FutureType",
            "_root_.golem.schema.SchemaTypeBody.StreamType",
            "fixture.Recursive",
        ] {
            assert!(source.contains(expected), "missing {expected}:\n{source}");
        }
        assert!(source.contains(r#"quote \" slash \\"#));
    }

    #[test]
    fn registry_deduplicates_exact_graphs_and_emits_lazy_vals() {
        let realistic = realistic_schema_graph();
        let exhaustive = exhaustive_schema_graph();
        let mut registry = SchemaGraphRegistry::default();

        assert_eq!(
            registry.intern(realistic.clone()),
            "__GolemSchemaGraphs.graph0"
        );
        assert_eq!(registry.intern(realistic), "__GolemSchemaGraphs.graph0");
        assert_eq!(registry.intern(exhaustive), "__GolemSchemaGraphs.graph1");

        let definition = registry.definition().unwrap();
        assert_eq!(definition.matches("lazy val graph").count(), 2);
        assert!(definition.starts_with("private object __GolemSchemaGraphs"));
        assert!(definition.contains("graph0"));
        assert!(definition.contains("graph1"));
    }
}

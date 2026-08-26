// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::metadata::{MetadataEnvelope, Role};
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NumericBound, NumericRestrictions, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, SchemaType, TextRestrictions,
    UrlRestrictions,
};

pub(crate) struct SchemaGraphRegistry {
    name: String,
    graphs: Vec<SchemaGraph>,
}

impl Default for SchemaGraphRegistry {
    fn default() -> Self {
        Self::new("__golemSchemaGraphs".to_string())
    }
}

impl SchemaGraphRegistry {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            graphs: Vec::new(),
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

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
        format!("{}.graph{index}()", self.name)
    }

    pub(crate) fn definitions(&self) -> String {
        let graphs = self
            .graphs
            .iter()
            .enumerate()
            .map(|(index, graph)| {
                format!(
                    "  graph{index}: (() => {{\n    let graph: base.SchemaGraph | undefined;\n    return (): base.SchemaGraph => graph ??= {};\n  }})(),",
                    emit_schema_graph_literal(graph)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !graphs.is_empty() {
            format!("const {} = {{\n{graphs}\n}};\n", self.name)
        } else {
            Default::default()
        }
    }
}

pub(crate) fn emit_schema_graph_literal(graph: &SchemaGraph) -> String {
    let defs = graph
        .defs
        .iter()
        .map(|def| {
            format!(
                "[{}, {{ name: {}, body: {} }}]",
                string(def.id.as_str()),
                optional_string(def.name.as_deref()),
                emit_schema_type(&def.body)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{ defs: new Map<string, base.SchemaTypeDef>([{defs}]), root: {} }}",
        emit_schema_type(&graph.root)
    )
}

fn emit_schema_type(typ: &SchemaType) -> String {
    use SchemaType::*;

    let body = match typ {
        Ref { id, .. } => format!("{{ tag: 'ref', id: {} }}", string(id.as_str())),
        Bool { .. } => "{ tag: 'bool' }".to_string(),
        S8 { restrictions, .. } => numeric("s8", restrictions.as_ref()),
        S16 { restrictions, .. } => numeric("s16", restrictions.as_ref()),
        S32 { restrictions, .. } => numeric("s32", restrictions.as_ref()),
        S64 { restrictions, .. } => numeric("s64", restrictions.as_ref()),
        U8 { restrictions, .. } => numeric("u8", restrictions.as_ref()),
        U16 { restrictions, .. } => numeric("u16", restrictions.as_ref()),
        U32 { restrictions, .. } => numeric("u32", restrictions.as_ref()),
        U64 { restrictions, .. } => numeric("u64", restrictions.as_ref()),
        F32 { restrictions, .. } => numeric("f32", restrictions.as_ref()),
        F64 { restrictions, .. } => numeric("f64", restrictions.as_ref()),
        Char { .. } => "{ tag: 'char' }".to_string(),
        String { .. } => "{ tag: 'string' }".to_string(),
        Record { fields, .. } => format!(
            "{{ tag: 'record', fields: [{}] }}",
            fields
                .iter()
                .map(|field| format!(
                    "{{ name: {}, body: {}, metadata: {} }}",
                    string(&field.name),
                    emit_schema_type(&field.body),
                    metadata(&field.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Variant { cases, .. } => format!(
            "{{ tag: 'variant', cases: [{}] }}",
            cases
                .iter()
                .map(|case| format!(
                    "{{ name: {}, payload: {}, metadata: {} }}",
                    string(&case.name),
                    optional_type(case.payload.as_ref()),
                    metadata(&case.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Enum { cases, .. } => format!("{{ tag: 'enum', cases: {} }}", strings(cases)),
        Flags { flags, .. } => format!("{{ tag: 'flags', names: {} }}", strings(flags)),
        Tuple { elements, .. } => format!(
            "{{ tag: 'tuple', elements: [{}] }}",
            elements
                .iter()
                .map(emit_schema_type)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        List { element, .. } => {
            format!("{{ tag: 'list', element: {} }}", emit_schema_type(element))
        }
        FixedList {
            element, length, ..
        } => format!(
            "{{ tag: 'fixed-list', element: {}, length: {length} }}",
            emit_schema_type(element)
        ),
        Map { key, value, .. } => format!(
            "{{ tag: 'map', key: {}, value: {} }}",
            emit_schema_type(key),
            emit_schema_type(value)
        ),
        Option { inner, .. } => {
            format!("{{ tag: 'option', element: {} }}", emit_schema_type(inner))
        }
        Result { spec, .. } => format!(
            "{{ tag: 'result', ok: {}, err: {} }}",
            optional_type(spec.ok.as_deref()),
            optional_type(spec.err.as_deref())
        ),
        Text { restrictions, .. } => format!(
            "{{ tag: 'text', restrictions: {} }}",
            text_restrictions(restrictions)
        ),
        Binary { restrictions, .. } => format!(
            "{{ tag: 'binary', restrictions: {} }}",
            binary_restrictions(restrictions)
        ),
        Path { spec, .. } => format!("{{ tag: 'path', spec: {} }}", path_spec(spec)),
        Url { restrictions, .. } => format!(
            "{{ tag: 'url', restrictions: {} }}",
            url_restrictions(restrictions)
        ),
        Datetime { .. } => "{ tag: 'datetime' }".to_string(),
        Duration { .. } => "{ tag: 'duration' }".to_string(),
        Quantity { spec, .. } => {
            format!("{{ tag: 'quantity', spec: {} }}", quantity_spec(spec))
        }
        Union { spec, .. } => format!(
            "{{ tag: 'union', branches: [{}] }}",
            spec.branches
                .iter()
                .map(|branch| format!(
                    "{{ tag: {}, body: {}, discriminator: {}, metadata: {} }}",
                    string(&branch.tag),
                    emit_schema_type(&branch.body),
                    discriminator(&branch.discriminator),
                    metadata(&branch.metadata)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Secret { spec, .. } => format!(
            "{{ tag: 'secret', spec: {{ category: {} }}, inner: {} }}",
            optional_string(spec.category.as_deref()),
            emit_schema_type(&spec.inner)
        ),
        QuotaToken { spec, .. } => {
            format!("{{ tag: 'quota-token', spec: {} }}", quota_token_spec(spec))
        }
        PermissionCard { spec, .. } => format!(
            "{{ tag: 'permission-card', spec: {{ polymorphic: {} }} }}",
            spec.polymorphic
        ),
        Future { inner, .. } => format!(
            "{{ tag: 'future', element: {} }}",
            optional_type(inner.as_deref())
        ),
        Stream { inner, .. } => format!(
            "{{ tag: 'stream', element: {} }}",
            optional_type(inner.as_deref())
        ),
    };

    if typ.metadata().is_empty() {
        format!("base.schemaType({body})")
    } else {
        format!("base.schemaType({body}, {})", metadata(typ.metadata()))
    }
}

fn numeric(tag: &str, restrictions: Option<&NumericRestrictions>) -> String {
    format!(
        "{{ tag: '{tag}', restrictions: {} }}",
        restrictions
            .map(numeric_restrictions)
            .unwrap_or_else(|| "undefined".to_string())
    )
}

fn numeric_restrictions(value: &NumericRestrictions) -> String {
    format!(
        "{{ min: {}, max: {}, unit: {} }}",
        optional_bound(value.min),
        optional_bound(value.max),
        optional_string(value.unit.as_deref())
    )
}

fn optional_bound(value: Option<NumericBound>) -> String {
    value.map_or_else(
        || "undefined".to_string(),
        |value| match value {
            NumericBound::Signed(value) => format!("{{ tag: 'signed', val: {value}n }}"),
            NumericBound::Unsigned(value) => format!("{{ tag: 'unsigned', val: {value}n }}"),
            NumericBound::FloatBits(value) => format!("{{ tag: 'float-bits', val: {value}n }}"),
        },
    )
}

fn metadata(value: &MetadataEnvelope) -> String {
    let role = value.role.as_ref().map_or_else(
        || "undefined".to_string(),
        |role| match role {
            Role::Multimodal => "{ tag: 'multimodal' }".to_string(),
            Role::UnstructuredText => "{ tag: 'unstructured-text' }".to_string(),
            Role::UnstructuredBinary => "{ tag: 'unstructured-binary' }".to_string(),
            Role::Other(value) => format!("{{ tag: 'other', val: {} }}", string(value)),
        },
    );
    format!(
        "{{ doc: {}, aliases: {}, examples: {}, deprecated: {}, role: {role} }}",
        optional_string(value.doc.as_deref()),
        strings(&value.aliases),
        strings(&value.examples),
        optional_string(value.deprecated.as_deref())
    )
}

fn text_restrictions(value: &TextRestrictions) -> String {
    format!(
        "{{ languages: {}, minLength: {}, maxLength: {}, regex: {} }}",
        optional_strings(value.languages.as_deref()),
        optional_number(value.min_length),
        optional_number(value.max_length),
        optional_string(value.regex.as_deref())
    )
}

fn binary_restrictions(value: &BinaryRestrictions) -> String {
    format!(
        "{{ mimeTypes: {}, minBytes: {}, maxBytes: {} }}",
        optional_strings(value.mime_types.as_deref()),
        optional_number(value.min_bytes),
        optional_number(value.max_bytes)
    )
}

fn path_spec(value: &PathSpec) -> String {
    let direction = match value.direction {
        PathDirection::Input => "input",
        PathDirection::Output => "output",
        PathDirection::InOut => "in-out",
    };
    let kind = match value.kind {
        PathKind::File => "file",
        PathKind::Directory => "directory",
        PathKind::Any => "any",
    };
    format!(
        "{{ direction: '{direction}', kind: '{kind}', allowedMimeTypes: {}, allowedExtensions: {} }}",
        optional_strings(value.allowed_mime_types.as_deref()),
        optional_strings(value.allowed_extensions.as_deref())
    )
}

fn url_restrictions(value: &UrlRestrictions) -> String {
    format!(
        "{{ allowedSchemes: {}, allowedHosts: {} }}",
        optional_strings(value.allowed_schemes.as_deref()),
        optional_strings(value.allowed_hosts.as_deref())
    )
}

fn quantity_spec(value: &QuantitySpec) -> String {
    format!(
        "{{ baseUnit: {}, allowedSuffixes: {}, min: {}, max: {} }}",
        string(&value.base_unit),
        strings(&value.allowed_suffixes),
        optional_quantity(value.min.as_ref()),
        optional_quantity(value.max.as_ref())
    )
}

fn optional_quantity(value: Option<&QuantityValue>) -> String {
    value.map_or_else(
        || "undefined".to_string(),
        |value| {
            format!(
                "{{ mantissa: {}n, scale: {}, unit: {} }}",
                value.mantissa,
                value.scale,
                string(&value.unit)
            )
        },
    )
}

fn discriminator(value: &DiscriminatorRule) -> String {
    match value {
        DiscriminatorRule::Prefix { prefix } => {
            format!("{{ tag: 'prefix', val: {} }}", string(prefix))
        }
        DiscriminatorRule::Suffix { suffix } => {
            format!("{{ tag: 'suffix', val: {} }}", string(suffix))
        }
        DiscriminatorRule::Contains { substring } => {
            format!("{{ tag: 'contains', val: {} }}", string(substring))
        }
        DiscriminatorRule::Regex { regex } => {
            format!("{{ tag: 'regex', val: {} }}", string(regex))
        }
        DiscriminatorRule::FieldEquals(field) => format!(
            "{{ tag: 'field-equals', val: {{ fieldName: {}, literal: {} }} }}",
            string(&field.field_name),
            optional_string(field.literal.as_deref())
        ),
        DiscriminatorRule::FieldAbsent { field_name } => {
            format!("{{ tag: 'field-absent', val: {} }}", string(field_name))
        }
    }
}

fn quota_token_spec(value: &QuotaTokenSpec) -> String {
    format!(
        "{{ resourceName: {} }}",
        optional_string(value.resource_name.as_deref())
    )
}

fn optional_type(value: Option<&SchemaType>) -> String {
    value
        .map(emit_schema_type)
        .unwrap_or_else(|| "undefined".to_string())
}

fn optional_string(value: Option<&str>) -> String {
    value.map(string).unwrap_or_else(|| "undefined".to_string())
}

fn optional_strings(value: Option<&[String]>) -> String {
    value
        .map(strings)
        .unwrap_or_else(|| "undefined".to_string())
}

fn optional_number<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "undefined".to_string())
}

fn strings(values: &[String]) -> String {
    serde_json::to_string(values).expect("strings always serialize")
}

fn string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_gen::schema_graph_test_fixture::{
        exhaustive_schema_graph, realistic_schema_graph,
    };
    use test_r::test;

    #[test]
    fn exhaustive_literal_is_deterministic_and_preserves_edges() {
        let graph = exhaustive_schema_graph();
        let source = emit_schema_graph_literal(&graph);

        assert_eq!(source, emit_schema_graph_literal(&graph));
        for expected in [
            "-9223372036854775808n",
            "9223372036854775807n",
            "18446744073709551615n",
            "9223372036854775808n",
            "mantissa: -9223372036854775808n",
            "scale: -2147483648",
            "length: 4294967295",
            "tag: 'multimodal'",
            "tag: 'unstructured-text'",
            "tag: 'unstructured-binary'",
            "tag: 'other'",
            "tag: 'field-equals'",
            "tag: 'field-absent'",
            "tag: 'future'",
            "tag: 'stream'",
            "fixture.Recursive",
        ] {
            assert!(source.contains(expected), "missing {expected}:\n{source}");
        }
        assert!(source.contains(r#"quote \" slash \\"#));
    }

    #[test]
    fn registry_deduplicates_exact_graphs_and_emits_memoized_getters() {
        let realistic = realistic_schema_graph();
        let exhaustive = exhaustive_schema_graph();
        let mut registry = SchemaGraphRegistry::default();

        assert_eq!(
            registry.intern(realistic.clone()),
            "__golemSchemaGraphs.graph0()"
        );
        assert_eq!(registry.intern(realistic), "__golemSchemaGraphs.graph0()");
        assert_eq!(registry.intern(exhaustive), "__golemSchemaGraphs.graph1()");

        let definitions = registry.definitions();
        assert!(definitions.starts_with("const __golemSchemaGraphs = {"));
        assert_eq!(
            definitions.matches("return (): base.SchemaGraph").count(),
            2
        );
        assert_eq!(definitions.matches("??=").count(), 2);
        assert!(definitions.contains("graph0:"));
        assert!(definitions.contains("graph1:"));
    }
}

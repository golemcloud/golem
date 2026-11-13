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

//! Schemas held as data: kinds, names, known roles and declared quantity units, without
//! metadata texts, restrictions, discriminator literals and capability specs.

use super::value::{number, record, text};
use crate::oplog_export::model::{
    KnownRole, NamedTypeDescription, ReportField, ReportNode, TupleNode, TypeDescription,
    TypeDescriptionNode, TypeKind,
};
use golem_common::schema::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema, InputSchema,
    NamedField, OutputSchema, RegisteredAgentTypeSchema, Role, SchemaGraph, SchemaType,
};

pub fn describe_schema(graph: &SchemaGraph) -> TypeDescription {
    let mut root = describe(&graph.root);
    root.definitions = graph
        .defs
        .iter()
        .map(|def| NamedTypeDescription {
            name: def.id.to_string(),
            body: Some(TypeDescription {
                type_name: def.name.clone(),
                ..describe(&def.body)
            }),
        })
        .collect();
    root
}

pub(super) fn known_role(role: &Role) -> Option<KnownRole> {
    match role {
        Role::Multimodal => Some(KnownRole::Multimodal),
        Role::UnstructuredText => Some(KnownRole::UnstructuredText),
        Role::UnstructuredBinary => Some(KnownRole::UnstructuredBinary),
        Role::Other(_) => None,
    }
}

fn leaf(kind: TypeKind) -> TypeDescription {
    TypeDescription {
        kind,
        type_name: None,
        role: None,
        members: Vec::new(),
        elements: Vec::new(),
        names: Vec::new(),
        base_unit: None,
        allowed_suffixes: Vec::new(),
        definitions: Vec::new(),
    }
}

fn describe(ty: &SchemaType) -> TypeDescription {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let mut description = match ty {
            SchemaType::Ref { id, .. } => TypeDescription {
                type_name: Some(id.to_string()),
                ..leaf(TypeKind::Ref)
            },
            SchemaType::Bool { .. } => leaf(TypeKind::Bool),
            SchemaType::S8 { .. } => leaf(TypeKind::S8),
            SchemaType::S16 { .. } => leaf(TypeKind::S16),
            SchemaType::S32 { .. } => leaf(TypeKind::S32),
            SchemaType::S64 { .. } => leaf(TypeKind::S64),
            SchemaType::U8 { .. } => leaf(TypeKind::U8),
            SchemaType::U16 { .. } => leaf(TypeKind::U16),
            SchemaType::U32 { .. } => leaf(TypeKind::U32),
            SchemaType::U64 { .. } => leaf(TypeKind::U64),
            SchemaType::F32 { .. } => leaf(TypeKind::F32),
            SchemaType::F64 { .. } => leaf(TypeKind::F64),
            SchemaType::Char { .. } => leaf(TypeKind::Char),
            SchemaType::String { .. } => leaf(TypeKind::String),
            SchemaType::Record { fields, .. } => TypeDescription {
                members: fields
                    .iter()
                    .map(|field| NamedTypeDescription {
                        name: field.name.clone(),
                        body: Some(describe(&field.body)),
                    })
                    .collect(),
                ..leaf(TypeKind::Record)
            },
            SchemaType::Variant { cases, .. } => TypeDescription {
                members: cases
                    .iter()
                    .map(|case| NamedTypeDescription {
                        name: case.name.clone(),
                        body: case.payload.as_ref().map(describe),
                    })
                    .collect(),
                ..leaf(TypeKind::Variant)
            },
            SchemaType::Enum { cases, .. } => TypeDescription {
                names: cases.clone(),
                ..leaf(TypeKind::Enum)
            },
            SchemaType::Flags { flags, .. } => TypeDescription {
                names: flags.clone(),
                ..leaf(TypeKind::Flags)
            },
            SchemaType::Tuple { elements, .. } => TypeDescription {
                elements: elements.iter().map(describe).collect(),
                ..leaf(TypeKind::Tuple)
            },
            SchemaType::List { element, .. } => TypeDescription {
                elements: vec![describe(element)],
                ..leaf(TypeKind::List)
            },
            SchemaType::FixedList { element, .. } => TypeDescription {
                elements: vec![describe(element)],
                ..leaf(TypeKind::FixedList)
            },
            SchemaType::Map { key, value, .. } => TypeDescription {
                elements: vec![describe(key), describe(value)],
                ..leaf(TypeKind::Map)
            },
            SchemaType::Option { inner, .. } => TypeDescription {
                elements: vec![describe(inner)],
                ..leaf(TypeKind::Option)
            },
            SchemaType::Result { spec, .. } => TypeDescription {
                members: [("ok", spec.ok.as_deref()), ("err", spec.err.as_deref())]
                    .into_iter()
                    .map(|(name, body)| NamedTypeDescription {
                        name: name.to_string(),
                        body: body.map(describe),
                    })
                    .collect(),
                ..leaf(TypeKind::Result)
            },
            SchemaType::Text { .. } => leaf(TypeKind::Text),
            SchemaType::Binary { .. } => leaf(TypeKind::Binary),
            SchemaType::Path { .. } => leaf(TypeKind::Path),
            SchemaType::Url { .. } => leaf(TypeKind::Url),
            SchemaType::Uuid { .. } => leaf(TypeKind::Uuid),
            SchemaType::Datetime { .. } => leaf(TypeKind::Datetime),
            SchemaType::Duration { .. } => leaf(TypeKind::Duration),
            SchemaType::Quantity { spec, .. } => TypeDescription {
                base_unit: Some(spec.base_unit.clone()),
                allowed_suffixes: spec.allowed_suffixes.clone(),
                ..leaf(TypeKind::Quantity)
            },
            SchemaType::Union { spec, .. } => TypeDescription {
                members: spec
                    .branches
                    .iter()
                    .map(|branch| NamedTypeDescription {
                        name: branch.tag.clone(),
                        body: Some(describe(&branch.body)),
                    })
                    .collect(),
                ..leaf(TypeKind::Union)
            },
            SchemaType::Secret { spec, .. } => TypeDescription {
                elements: vec![describe(&spec.inner)],
                ..leaf(TypeKind::Secret)
            },
            SchemaType::QuotaToken { .. } => leaf(TypeKind::QuotaToken),
            SchemaType::PermissionCard { .. } => leaf(TypeKind::PermissionCard),
            SchemaType::Future { inner, .. } => TypeDescription {
                elements: inner.as_deref().map(describe).into_iter().collect(),
                ..leaf(TypeKind::Future)
            },
            SchemaType::Stream { inner, .. } => TypeDescription {
                elements: inner.as_deref().map(describe).into_iter().collect(),
                ..leaf(TypeKind::Stream)
            },
        };
        description.role = ty.metadata().role.as_ref().and_then(known_role);
        description
    })
}

fn field(name: &str, value: ReportNode) -> ReportField {
    ReportField {
        name: Some(name.to_string()),
        value,
    }
}

fn type_node(description: TypeDescription) -> ReportNode {
    ReportNode::TypeDescription(TypeDescriptionNode { description })
}

fn tuple(elements: Vec<ReportNode>) -> ReportNode {
    ReportNode::Tuple(TupleNode { elements })
}

/// A registered agent type held as data: the agent type, constructor, method, parameter and
/// dependency names with their type descriptions, and the component revision. Descriptions,
/// prompt hints, HTTP and configuration details, and the implementing component and account
/// are removed.
pub fn describe_agent_type(registered: &RegisteredAgentTypeSchema) -> ReportNode {
    let RegisteredAgentTypeSchema {
        agent_type,
        implemented_by,
    } = registered;
    let AgentTypeSchema {
        type_name,
        kind: _,
        description: _,
        source_language: _,
        schema,
        constructor,
        methods,
        dependencies,
        mode: _,
        http_mount: _,
        snapshotting: _,
        config: _,
    } = agent_type;
    let agent_type = record(
        None,
        vec![
            field("typeName", text(type_name.0.clone())),
            field("schema", type_node(describe_schema(schema))),
            field("constructor", describe_constructor(constructor)),
            field(
                "methods",
                tuple(methods.iter().map(describe_method).collect()),
            ),
            field(
                "dependencies",
                tuple(dependencies.iter().map(describe_dependency).collect()),
            ),
        ],
    );
    record(
        None,
        vec![
            field("agentType", agent_type),
            field(
                "implementedBy",
                record(
                    None,
                    vec![field(
                        "componentRevision",
                        number(implemented_by.component_revision.get()),
                    )],
                ),
            ),
        ],
    )
}

fn describe_dependency(dependency: &AgentDependencySchema) -> ReportNode {
    let AgentDependencySchema {
        type_name,
        description: _,
        schema,
        constructor,
        methods,
    } = dependency;
    record(
        None,
        vec![
            field("typeName", text(type_name.clone())),
            field("schema", type_node(describe_schema(schema))),
            field("constructor", describe_constructor(constructor)),
            field(
                "methods",
                tuple(methods.iter().map(describe_method).collect()),
            ),
        ],
    )
}

fn describe_constructor(constructor: &AgentConstructorSchema) -> ReportNode {
    let AgentConstructorSchema {
        name,
        description: _,
        prompt_hint: _,
        input_schema,
    } = constructor;
    let mut fields = Vec::new();
    if let Some(name) = name {
        fields.push(field("name", text(name.clone())));
    }
    fields.push(field("inputSchema", describe_input(input_schema)));
    record(None, fields)
}

fn describe_method(method: &AgentMethodSchema) -> ReportNode {
    let AgentMethodSchema {
        name,
        description: _,
        prompt_hint: _,
        input_schema,
        output_schema,
        http_endpoint: _,
        read_only: _,
    } = method;
    let mut fields = vec![
        field("name", text(name.clone())),
        field("inputSchema", describe_input(input_schema)),
    ];
    match output_schema {
        OutputSchema::Unit => {}
        OutputSchema::Single(ty) => fields.push(field("outputSchema", type_node(describe(ty)))),
    }
    record(None, fields)
}

fn describe_input(input: &InputSchema) -> ReportNode {
    match input {
        InputSchema::Parameters(parameters) => tuple(
            parameters
                .iter()
                .map(|parameter| {
                    let NamedField {
                        name,
                        source: _,
                        schema,
                        metadata: _,
                    } = parameter;
                    record(
                        None,
                        vec![
                            field("name", text(name.clone())),
                            field("schema", type_node(describe(schema))),
                        ],
                    )
                })
                .collect(),
        ),
    }
}

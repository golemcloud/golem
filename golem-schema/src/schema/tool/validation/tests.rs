use super::*;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{NamedFieldType, VariantCaseType};
use crate::schema::tool::{CommandTree, Doc, MonomorphicToolMiddlewareScope, ToolMiddlewareScope};
use test_r::test;

fn tool(name: &str) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: name.to_string(),
                aliases: vec![],
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: vec![],
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

#[test]
fn rejects_malformed_middleware_identity_and_both_embedded_tools() {
    let middleware = ToolMiddleware {
        name: "Bad_Name".to_string(),
        version: "middleware-v3".to_string(),
        aliases: vec!["Bad_Name".to_string()],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
            presented: tool("BadPresented"),
            expected: Some(tool("BadExpected")),
        })),
        parameter_schema: SchemaGraph::empty(),
    };

    let errors = validate_tool_middleware(&middleware).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolMiddlewareValidationError::InvalidIdentifier {
            kind: "tool middleware name",
            ..
        }
    )));
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolMiddlewareValidationError::DuplicateIdentity { .. }))
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolMiddlewareValidationError::InvalidPresentedTool(_)))
    );
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolMiddlewareValidationError::InvalidExpectedTool(_)))
    );
}

#[test]
fn rejects_empty_tool_and_middleware_versions() {
    let mut invalid_tool = tool("valid");
    invalid_tool.version.clear();
    assert!(
        validate_tool(&invalid_tool)
            .unwrap_err()
            .contains(&ToolValidationError::EmptyVersion)
    );
    let middleware = ToolMiddleware {
        name: "valid".into(),
        version: " ".into(),
        aliases: vec![],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Universal,
        parameter_schema: SchemaGraph::empty(),
    };
    assert_eq!(
        validate_tool_middleware(&middleware).unwrap_err(),
        vec![ToolMiddlewareValidationError::EmptyVersion]
    );
}

fn universal_middleware(parameter_schema: SchemaGraph) -> ToolMiddleware {
    ToolMiddleware {
        name: "valid".into(),
        version: "1".into(),
        aliases: vec![],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Universal,
        parameter_schema,
    }
}

#[test]
fn accepts_static_record_and_variant_parameter_schema() {
    let schema = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "mode".into(),
            body: SchemaType::Variant {
                cases: vec![VariantCaseType {
                    name: "named".into(),
                    payload: Some(SchemaType::string()),
                    metadata: Default::default(),
                }],
                metadata: Default::default(),
            },
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    });
    assert!(validate_tool_middleware(&universal_middleware(schema)).is_ok());
}

#[test]
fn rejects_nested_and_definition_host_or_async_parameter_types() {
    let schema = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("capability"),
            name: None,
            body: SchemaType::permission_card(Default::default()),
        }],
        root: SchemaType::tuple(vec![
            SchemaType::list(SchemaType::stream(Some(SchemaType::string()))),
            SchemaType::ref_to(TypeId::new("capability")),
        ]),
    };
    let errors = validate_tool_middleware(&universal_middleware(schema)).unwrap_err();
    assert!(
        errors.contains(&ToolMiddlewareValidationError::ForbiddenParameterType(
            "stream"
        ))
    );
    assert!(
        errors.contains(&ToolMiddlewareValidationError::ForbiddenParameterType(
            "permission-card"
        ))
    );
}

#[test]
fn rejects_malformed_parameter_graph() {
    let schema = SchemaGraph::anonymous(SchemaType::ref_to(TypeId::new("missing")));
    assert!(
        validate_tool_middleware(&universal_middleware(schema))
            .unwrap_err()
            .iter()
            .any(|error| matches!(
                error,
                ToolMiddlewareValidationError::InvalidParameterSchema(_)
            ))
    );
}

use super::*;
use crate::schema::graph::SchemaGraph;
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
    };
    assert_eq!(
        validate_tool_middleware(&middleware).unwrap_err(),
        vec![ToolMiddlewareValidationError::EmptyVersion]
    );
}

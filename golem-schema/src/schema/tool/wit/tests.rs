use super::*;
use crate::schema::graph::SchemaGraph;
use test_r::test;

fn tool(name: &str, version: &str) -> Tool {
    Tool {
        version: version.to_string(),
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
fn tool_middleware_roundtrip_preserves_independent_versions() {
    let middleware = ToolMiddleware {
        name: "redactor".to_string(),
        version: "middleware-7".to_string(),
        aliases: vec!["scrubber".to_string()],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
            presented: tool("public-tool", "presented-2"),
            expected: Some(tool("private-tool", "expected-9")),
        })),
    };

    let decoded = tool_middleware_from_wit(tool_middleware_to_wit(&middleware).unwrap()).unwrap();
    assert_eq!(decoded, middleware);
}

#[test]
fn universal_middleware_roundtrip_preserves_its_version() {
    let middleware = ToolMiddleware {
        name: "audit".to_string(),
        version: "middleware-only-version".to_string(),
        aliases: vec![],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Universal,
    };

    let decoded = tool_middleware_from_wit(tool_middleware_to_wit(&middleware).unwrap()).unwrap();
    assert_eq!(decoded, middleware);
}

#[test]
fn malformed_embedded_tool_is_rejected() {
    let middleware = ToolMiddleware {
        name: "redactor".to_string(),
        version: "1".to_string(),
        aliases: vec![],
        doc: Doc::default(),
        scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
            presented: tool("public-tool", "1"),
            expected: None,
        })),
    };
    let mut wire = tool_middleware_to_wit(&middleware).unwrap();
    let wire::ToolMiddlewareScope::Monomorphic(scope) = &mut wire.scope else {
        unreachable!()
    };
    scope.presented.schema.type_nodes[0].body =
        crate::schema::wit::wire::SchemaTypeBody::RefType(i32::MAX);
    scope
        .presented
        .schema
        .defs
        .push(crate::schema::wit::wire::SchemaTypeDef {
            id: "broken".to_string(),
            name: None,
            body: 0,
        });

    assert!(tool_middleware_from_wit(wire).is_err());
}

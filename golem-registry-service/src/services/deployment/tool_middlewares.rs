// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use golem_common::model::agent::AgentTypeName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::tool::{CompiledToolBinding, RegisteredTool, ToolBindingInput, ToolName};
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareInstallation, ToolMiddlewareMergeMode,
};
use golem_common::schema::tool::compatibility::{
    ToolCompatibilityMode, compile_tool_compatibility,
};
use golem_common::schema::tool::validation::{validate_tool, validate_tool_middleware};
use golem_common::schema::tool::{ErrorCase, Tool, ToolMiddlewareScope};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMiddlewareCompileDiagnostic {
    pub agent_type_name: AgentTypeName,
    pub tool_name: ToolName,
    pub middleware_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledToolMiddlewareChains {
    pub chains: Vec<CompiledToolMiddlewareChain>,
    pub warnings: Vec<ToolMiddlewareCompileDiagnostic>,
    pub errors: Vec<ToolMiddlewareCompileDiagnostic>,
}

/// Purely compiles pinned middleware chains. It performs no repository access and has no
/// deployment side effects.
#[allow(clippy::too_many_arguments)]
pub fn compile_tool_middleware_chains(
    deployment_revision: DeploymentRevision,
    registered_tools: &[RegisteredTool],
    agent_tool_bindings: &[CompiledToolBinding],
    middleware_registrations: &[RegisteredToolMiddleware],
    universal_installations: &[ToolMiddlewareInstallation],
    environment_tool_bindings: &BTreeMap<ToolName, ToolBindingInput>,
    agent_tool_binding_inputs: &BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
    compatibility_mode: ToolCompatibilityMode,
) -> CompiledToolMiddlewareChains {
    let tools = registered_tools
        .iter()
        .filter_map(|tool| {
            tool.definition
                .name()
                .and_then(|name| ToolName::try_from(name).ok())
                .map(|name| (name, tool))
        })
        .collect::<BTreeMap<_, _>>();
    let warnings = Vec::new();
    let mut errors = Vec::new();
    let mut chains = Vec::new();
    let mut invalid_registry = false;

    // Releases are durable inputs and cannot be assumed to have passed the current validator.
    // Validate the complete registries, including entries not selected by a binding.
    for binding in agent_tool_bindings {
        for tool in registered_tools {
            if let Err(validation_errors) = validate_tool(&tool.definition) {
                invalid_registry = true;
                for error in validation_errors {
                    errors.push(diagnostic(
                        binding,
                        None,
                        format!("invalid registered tool descriptor: {error}"),
                    ));
                }
            }
        }
        for middleware in middleware_registrations {
            if let Err(validation_errors) = validate_tool_middleware(&middleware.definition) {
                invalid_registry = true;
                for error in validation_errors {
                    errors.push(diagnostic(
                        binding,
                        Some(middleware.definition.name.clone()),
                        format!("invalid registered middleware descriptor: {error}"),
                    ));
                }
            }
        }
        let mut identities = BTreeMap::<&str, &str>::new();
        for middleware in middleware_registrations {
            for identity in std::iter::once(middleware.definition.name.as_str())
                .chain(middleware.definition.aliases.iter().map(String::as_str))
            {
                if let Some(first) = identities.insert(identity, &middleware.definition.name) {
                    invalid_registry = true;
                    errors.push(diagnostic(
                        binding,
                        Some(middleware.definition.name.clone()),
                        format!(
                            "duplicate middleware registry identity `{identity}` (also registered by `{first}`)"
                        ),
                    ));
                }
            }
        }
    }

    if invalid_registry {
        return CompiledToolMiddlewareChains {
            chains,
            warnings,
            errors,
        };
    }

    for binding in agent_tool_bindings {
        let Some(tool) = tools.get(&binding.tool_name) else {
            errors.push(diagnostic(
                binding,
                None,
                "compiled binding has no registered leaf tool",
            ));
            continue;
        };
        let environment = environment_tool_bindings.get(&binding.tool_name);
        let agent = agent_tool_binding_inputs
            .get(&binding.agent_type_name)
            .and_then(|bindings| bindings.get(&binding.tool_name));
        let per_tool = effective_installations(environment, agent);
        let mut resolved = Vec::new();
        let mut valid = true;
        for (installation, universal) in universal_installations
            .iter()
            .map(|i| (i, true))
            .chain(per_tool.iter().map(|i| (i, false)))
        {
            match resolve_registration(installation, middleware_registrations, universal) {
                Ok(registration) => resolved.push((installation, registration, universal)),
                Err(message) => {
                    errors.push(diagnostic(
                        binding,
                        Some(installation.name.to_string()),
                        message,
                    ));
                    valid = false;
                }
            }
        }
        if !valid {
            continue;
        }

        let universal_count = universal_installations.len();
        let mut effective = tool.definition.clone();
        let mut compiled_reversed = Vec::with_capacity(resolved.len());
        for (installation, registration, universal) in resolved.iter().rev() {
            let next = effective.clone();
            let (expected, presented, compatibility, next_effective) = match &registration
                .definition
                .scope
            {
                ToolMiddlewareScope::Universal if *universal => (None, None, None, next.clone()),
                ToolMiddlewareScope::Monomorphic(scope) if !*universal => {
                    let expected = scope.expected.clone();
                    let compatibility = match &expected {
                        Some(expected) => {
                            match compile_tool_compatibility(expected, &next, compatibility_mode) {
                                Ok(compiled) => Some(compiled),
                                Err(es) => {
                                    for error in es {
                                        errors.push(diagnostic(
                                            binding,
                                            Some(installation.name.to_string()),
                                            format!("{}: {}", error.path, error.message),
                                        ));
                                    }
                                    valid = false;
                                    None
                                }
                            }
                        }
                        None => None,
                    };
                    let synthesized = match synthesize_effective_definition(
                        &scope.presented,
                        expected.as_ref(),
                        &next,
                    ) {
                        Ok(tool) => tool,
                        Err(message) => {
                            errors.push(diagnostic(
                                binding,
                                Some(installation.name.to_string()),
                                message,
                            ));
                            valid = false;
                            scope.presented.clone()
                        }
                    };
                    (
                        expected,
                        Some(scope.presented.clone()),
                        compatibility,
                        synthesized,
                    )
                }
                _ => unreachable!("scope was checked while resolving"),
            };
            effective = next_effective;
            compiled_reversed.push(CompiledToolMiddlewareOccurrence {
                middleware: (*registration).clone(),
                parameters: installation.parameters.clone(),
                provision: registration.provision.clone(),
                secret_keys_readable: binding.secret_keys_readable.clone(),
                secret_keys_revealable: binding.secret_keys_revealable.clone(),
                filesystem_access: installation.filesystem_access,
                expected_definition: expected,
                presented_definition: presented,
                next_effective_definition: next,
                compatibility,
            });
        }
        if !valid {
            continue;
        }
        compiled_reversed.reverse();
        debug_assert_eq!(compiled_reversed.len(), universal_count + per_tool.len());
        chains.push(CompiledToolMiddlewareChain {
            deployment_revision,
            agent_type_name: binding.agent_type_name.clone(),
            tool_name: binding.tool_name.clone(),
            effective_definition: effective,
            occurrences: compiled_reversed,
        });
    }
    CompiledToolMiddlewareChains {
        chains,
        warnings,
        errors,
    }
}

fn effective_installations(
    environment: Option<&ToolBindingInput>,
    agent: Option<&ToolBindingInput>,
) -> Vec<ToolMiddlewareInstallation> {
    let environment = environment
        .and_then(|b| b.middleware.as_ref())
        .cloned()
        .unwrap_or_default();
    let Some(agent_binding) = agent else {
        return environment;
    };
    let Some(agent_installations) = &agent_binding.middleware else {
        return environment;
    };
    match agent_binding.middleware_merge_mode {
        ToolMiddlewareMergeMode::Prepend => agent_installations
            .iter()
            .chain(&environment)
            .cloned()
            .collect(),
        ToolMiddlewareMergeMode::Append => environment
            .iter()
            .chain(agent_installations)
            .cloned()
            .collect(),
        ToolMiddlewareMergeMode::Replace => agent_installations.clone(),
    }
}

fn resolve_registration<'a>(
    installation: &ToolMiddlewareInstallation,
    registrations: &'a [RegisteredToolMiddleware],
    universal: bool,
) -> Result<&'a RegisteredToolMiddleware, String> {
    let matches = registrations
        .iter()
        .filter(|r| {
            r.definition.name == installation.name.as_str()
                || r.definition
                    .aliases
                    .iter()
                    .any(|a| a == installation.name.as_str())
        })
        .collect::<Vec<_>>();
    let [registration] = matches.as_slice() else {
        return Err(if matches.is_empty() {
            "unresolved middleware name"
        } else {
            "duplicate middleware name"
        }
        .to_string());
    };
    if installation
        .version
        .as_ref()
        .is_some_and(|v| v != &registration.definition.version)
    {
        return Err(format!(
            "requested version does not match definition version {}",
            registration.definition.version
        ));
    }
    if installation
        .account
        .as_ref()
        .is_some_and(|a| a != &registration.owner_account_email)
    {
        return Err(format!(
            "requested account does not match owner {}",
            registration.owner_account_email
        ));
    }
    if universal
        != matches!(
            registration.definition.scope,
            ToolMiddlewareScope::Universal
        )
    {
        return Err("middleware is installed in the wrong scope".to_string());
    }
    Ok(registration)
}

fn synthesize_effective_definition(
    presented: &Tool,
    expected: Option<&Tool>,
    next: &Tool,
) -> Result<Tool, String> {
    let mut result = presented.clone();
    let expected_known = expected.map(command_errors).unwrap_or_default();
    let next_errors = command_errors(next);
    let mut candidates = BTreeMap::<String, ErrorCase>::new();
    for (path, next_cases) in &next_errors {
        let known = expected_known.get(path);
        for inherited in next_cases {
            if known.is_some_and(|known| known.iter().any(|item| item.name == inherited.name)) {
                continue;
            }
            if let Some(existing) = candidates.get(&inherited.name) {
                if existing.payload != inherited.payload {
                    return Err(format!(
                        "inherited error `{}` has different payload schemas in callable next commands",
                        inherited.name
                    ));
                }
            } else {
                candidates.insert(inherited.name.clone(), inherited.clone());
            }
        }
    }
    let mut imported_ids = BTreeMap::new();
    let paths = command_nodes(&result);
    for index in paths.values() {
        let Some(body) = result.commands.nodes[*index].body.as_mut() else {
            continue;
        };
        for inherited in candidates.values() {
            if let Some(existing) = body
                .errors
                .iter()
                .find(|error| error.name == inherited.name)
            {
                if existing.payload != inherited.payload {
                    return Err(format!(
                        "inherited error `{}` collides with a different payload schema",
                        inherited.name
                    ));
                }
            } else {
                let mut inherited = inherited.clone();
                if let Some(payload) = inherited.payload.as_mut() {
                    import_reachable_type(
                        payload,
                        &next.schema,
                        &mut result.schema,
                        &mut imported_ids,
                    )?;
                }
                body.errors.push(inherited);
            }
        }
    }
    if let Err(errors) = validate_tool(&result) {
        return Err(format!("invalid synthesized tool descriptor: {errors:?}"));
    }
    Ok(result)
}

fn import_reachable_type(
    ty: &mut SchemaType,
    source: &SchemaGraph,
    target: &mut SchemaGraph,
    imported: &mut BTreeMap<TypeId, TypeId>,
) -> Result<(), String> {
    rewrite_type_refs(ty, &mut |id| {
        import_definition(id, source, target, imported)
    })
}

fn import_definition(
    source_id: &TypeId,
    source: &SchemaGraph,
    target: &mut SchemaGraph,
    imported: &mut BTreeMap<TypeId, TypeId>,
) -> Result<TypeId, String> {
    if let Some(id) = imported.get(source_id) {
        return Ok(id.clone());
    }
    let source_def = source.lookup(source_id).ok_or_else(|| {
        format!("inherited error payload references missing definition `{source_id}`")
    })?;
    let occupied = target
        .defs
        .iter()
        .map(|def| def.id.clone())
        .chain(imported.values().cloned())
        .collect::<BTreeSet<_>>();
    let mut suffix = 0;
    let imported_id = loop {
        let candidate = TypeId::new(format!(
            "middleware-inherited-{suffix}-{}",
            source_id.as_str()
        ));
        if !occupied.contains(&candidate) {
            break candidate;
        }
        suffix += 1;
    };
    // Insert the mapping before descending so recursive definitions terminate.
    imported.insert(source_id.clone(), imported_id.clone());
    let mut body = source_def.body.clone();
    rewrite_type_refs(&mut body, &mut |id| {
        import_definition(id, source, target, imported)
    })?;
    target.defs.push(SchemaTypeDef {
        id: imported_id.clone(),
        name: source_def.name.clone(),
        body,
    });
    Ok(imported_id)
}

fn rewrite_type_refs(
    ty: &mut SchemaType,
    rewrite: &mut impl FnMut(&TypeId) -> Result<TypeId, String>,
) -> Result<(), String> {
    match ty {
        SchemaType::Ref { id, .. } => *id = rewrite(id)?,
        SchemaType::Record { fields, .. } => {
            for field in fields {
                rewrite_type_refs(&mut field.body, rewrite)?;
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(payload) = &mut case.payload {
                    rewrite_type_refs(payload, rewrite)?;
                }
            }
        }
        SchemaType::Tuple { elements, .. } => {
            for element in elements {
                rewrite_type_refs(element, rewrite)?;
            }
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            rewrite_type_refs(element, rewrite)?
        }
        SchemaType::Map { key, value, .. } => {
            rewrite_type_refs(key, rewrite)?;
            rewrite_type_refs(value, rewrite)?;
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: golem_common::schema::SecretSpec { inner, .. },
            ..
        } => rewrite_type_refs(inner, rewrite)?,
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &mut spec.ok {
                rewrite_type_refs(ok, rewrite)?;
            }
            if let Some(err) = &mut spec.err {
                rewrite_type_refs(err, rewrite)?;
            }
        }
        SchemaType::Union { spec, .. } => {
            for branch in &mut spec.branches {
                rewrite_type_refs(&mut branch.body, rewrite)?;
            }
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(inner) = inner {
                rewrite_type_refs(inner, rewrite)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn command_nodes(tool: &Tool) -> BTreeMap<Vec<String>, usize> {
    fn visit(
        tool: &Tool,
        index: usize,
        path: &mut Vec<String>,
        out: &mut BTreeMap<Vec<String>, usize>,
    ) {
        let Some(node) = tool.commands.nodes.get(index) else {
            return;
        };
        path.push(node.name.clone());
        if node.body.is_some() {
            out.insert(path.clone(), index);
        }
        let children = node.subcommands.clone();
        for child in children {
            if let Some(index) = child.as_usize() {
                visit(tool, index, path, out);
            }
        }
        path.pop();
    }
    let mut out = BTreeMap::new();
    visit(tool, 0, &mut Vec::new(), &mut out);
    out
}

fn command_errors(tool: &Tool) -> BTreeMap<Vec<String>, Vec<ErrorCase>> {
    command_nodes(tool)
        .into_iter()
        .filter_map(|(mut path, index)| {
            // Tool roots are presentation names, not part of the inner command vocabulary.
            path.remove(0);
            tool.commands.nodes[index]
                .body
                .as_ref()
                .map(|body| (path, body.errors.clone()))
        })
        .collect()
}

fn diagnostic(
    binding: &CompiledToolBinding,
    middleware_name: Option<String>,
    message: impl Into<String>,
) -> ToolMiddlewareCompileDiagnostic {
    ToolMiddlewareCompileDiagnostic {
        agent_type_name: binding.agent_type_name.clone(),
        tool_name: binding.tool_name.clone(),
        middleware_name,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_installations, synthesize_effective_definition};
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{ToolBindingInput, ToolFilesystemAccess};
    use golem_common::model::tool_middleware::{
        ToolMiddlewareInstallation, ToolMiddlewareMergeMode, ToolMiddlewareName,
    };
    use golem_common::schema::tool::{
        CommandBody, CommandIndex, CommandNode, CommandTree, Doc, ErrorCase, ErrorKind, Globals,
        Positionals, Tool,
    };
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
    use test_r::test;

    fn installation(name: &str, parameter: i32) -> ToolMiddlewareInstallation {
        ToolMiddlewareInstallation {
            name: ToolMiddlewareName::try_from(name).unwrap(),
            version: None,
            parameters: NormalizedJsonValue::new(serde_json::json!({ "value": parameter })),
            account: None,
            filesystem_access: Default::default(),
        }
    }

    fn binding(
        middleware: Option<Vec<ToolMiddlewareInstallation>>,
        middleware_merge_mode: ToolMiddlewareMergeMode,
    ) -> ToolBindingInput {
        ToolBindingInput {
            middleware,
            middleware_merge_mode,
            ..Default::default()
        }
    }

    fn rendered(chain: &[ToolMiddlewareInstallation]) -> Vec<(String, i64)> {
        chain
            .iter()
            .map(|item| {
                (
                    item.name.to_string(),
                    item.parameters.0["value"].as_i64().unwrap(),
                )
            })
            .collect()
    }

    fn error(name: &str, payload: Option<SchemaType>) -> ErrorCase {
        ErrorCase {
            name: name.to_string(),
            doc: Doc::default(),
            kind: ErrorKind::RuntimeError,
            exit_code: 1,
            payload,
        }
    }

    fn body(errors: Vec<ErrorCase>) -> CommandBody {
        CommandBody {
            positionals: Positionals::default(),
            options: Vec::new(),
            flags: Vec::new(),
            constraints: Vec::new(),
            stdin: None,
            stdout: None,
            result: None,
            errors,
            annotations: None,
        }
    }

    fn command(name: &str, errors: Vec<ErrorCase>) -> CommandNode {
        CommandNode {
            name: name.to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            globals: Globals::default(),
            subcommands: Vec::new(),
            body: Some(body(errors)),
        }
    }

    fn tool(root: &str, root_errors: Vec<ErrorCase>, subcommands: Vec<CommandNode>) -> Tool {
        let mut root = command(root, root_errors);
        root.subcommands = (1..=subcommands.len())
            .map(|index| CommandIndex(index as i32))
            .collect();
        Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: std::iter::once(root).chain(subcommands).collect(),
            },
            schema: SchemaGraph::empty(),
        }
    }

    #[test]
    fn per_tool_merge_preserves_omitted_empty_order_and_repetitions() {
        let environment = binding(
            Some(vec![
                installation("environment", 1),
                installation("same", 2),
            ]),
            ToolMiddlewareMergeMode::Prepend,
        );

        assert_eq!(
            rendered(&effective_installations(
                Some(&environment),
                Some(&binding(None, ToolMiddlewareMergeMode::Replace)),
            )),
            [("environment".into(), 1), ("same".into(), 2)]
        );
        assert!(
            effective_installations(
                Some(&environment),
                Some(&binding(Some(vec![]), ToolMiddlewareMergeMode::Replace)),
            )
            .is_empty()
        );

        let agent = vec![installation("same", 3), installation("agent", 4)];
        assert_eq!(
            rendered(&effective_installations(
                Some(&environment),
                Some(&binding(
                    Some(agent.clone()),
                    ToolMiddlewareMergeMode::Prepend
                )),
            )),
            [
                ("same".into(), 3),
                ("agent".into(), 4),
                ("environment".into(), 1),
                ("same".into(), 2),
            ]
        );
        assert_eq!(
            rendered(&effective_installations(
                Some(&environment),
                Some(&binding(
                    Some(agent.clone()),
                    ToolMiddlewareMergeMode::Append
                )),
            )),
            [
                ("environment".into(), 1),
                ("same".into(), 2),
                ("same".into(), 3),
                ("agent".into(), 4),
            ]
        );
        assert_eq!(
            rendered(&effective_installations(
                Some(&environment),
                Some(&binding(Some(agent), ToolMiddlewareMergeMode::Replace)),
            )),
            [("same".into(), 3), ("agent".into(), 4)]
        );
    }

    #[test]
    fn middleware_installation_owns_its_filesystem_policy() {
        let mut installation = installation("audit", 1);
        installation.filesystem_access = ToolFilesystemAccess::Denied;

        let effective = effective_installations(
            Some(&binding(
                Some(vec![installation]),
                ToolMiddlewareMergeMode::Prepend,
            )),
            None,
        );

        assert_eq!(effective[0].filesystem_access, ToolFilesystemAccess::Denied);
    }

    #[test]
    fn adapter_inherits_all_unknown_next_errors_without_using_presented_root_or_path() {
        let presented = tool("outward", Vec::new(), vec![command("visible", Vec::new())]);
        let expected = tool("expected-root", vec![error("known", None)], Vec::new());
        let next = tool(
            "inner-root",
            vec![error("known", None), error("shared", None)],
            vec![command("unrelated", vec![error("extra", None)])],
        );

        let synthesized =
            synthesize_effective_definition(&presented, Some(&expected), &next).unwrap();
        assert_eq!(synthesized.name(), Some("outward"));
        for node in &synthesized.commands.nodes {
            let names = node
                .body
                .as_ref()
                .unwrap()
                .errors
                .iter()
                .map(|case| case.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["extra", "shared"]);
        }
    }

    #[test]
    fn irrelevant_definition_collision_is_ignored_and_recursive_payload_is_imported_hygienically() {
        let mut presented = tool("outward", Vec::new(), Vec::new());
        presented.schema.defs.push(SchemaTypeDef {
            id: TypeId::new("node"),
            name: Some("Presented".to_string()),
            body: SchemaType::string(),
        });
        let mut next = tool(
            "inner",
            vec![error(
                "recursive",
                Some(SchemaType::ref_to(TypeId::new("node"))),
            )],
            Vec::new(),
        );
        next.schema.defs.push(SchemaTypeDef {
            id: TypeId::new("node"),
            name: Some("Recursive".to_string()),
            body: SchemaType::record(vec![golem_common::schema::NamedFieldType {
                name: "next".to_string(),
                body: SchemaType::option(SchemaType::ref_to(TypeId::new("node"))),
                metadata: Default::default(),
            }]),
        });
        next.schema.defs.push(SchemaTypeDef {
            id: TypeId::new("irrelevant"),
            name: None,
            body: SchemaType::u64(),
        });

        let synthesized = synthesize_effective_definition(&presented, None, &next).unwrap();
        assert_eq!(synthesized.schema.defs.len(), 2);
        assert_eq!(synthesized.schema.defs[0].id, TypeId::new("node"));
        assert_ne!(synthesized.schema.defs[1].id, TypeId::new("node"));
        assert!(
            synthesized
                .schema
                .defs
                .iter()
                .all(|definition| definition.id != TypeId::new("irrelevant"))
        );
    }

    #[test]
    fn inherited_error_payload_collision_is_rejected() {
        let presented = tool(
            "outward",
            vec![error("same", Some(SchemaType::string()))],
            Vec::new(),
        );
        let next = tool(
            "inner",
            vec![error("same", Some(SchemaType::u64()))],
            Vec::new(),
        );
        assert!(synthesize_effective_definition(&presented, None, &next).is_err());
    }
}

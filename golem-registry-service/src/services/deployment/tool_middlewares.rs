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
use golem_common::schema::validation::is_equivalent_cross_graph;
use golem_common::schema::{SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMiddlewareCompileDiagnostic {
    pub agent_type_name: Option<AgentTypeName>,
    pub tool_name: Option<ToolName>,
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
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    let mut chains = Vec::new();
    let mut invalid_registry = false;

    // Releases are durable inputs and cannot be assumed to have passed the current validator.
    // Validate the complete registries once, including entries not selected by a binding.
    for tool in registered_tools {
        if let Err(validation_errors) = validate_tool(&tool.definition) {
            invalid_registry = true;
            for error in validation_errors {
                errors.push(global_diagnostic(
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
                errors.push(global_diagnostic(
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
                errors.push(global_diagnostic(
                    Some(middleware.definition.name.clone()),
                    format!(
                        "duplicate middleware registry identity `{identity}` (also registered by `{first}`)"
                    ),
                ));
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
                                Ok(compiled) => {
                                    for warning in &compiled.warnings {
                                        warnings.push(diagnostic(
                                            binding,
                                            Some(installation.name.to_string()),
                                            format!(
                                                "{}: input `{}` is discarded before invoking the next tool",
                                                warning.path, warning.name
                                            ),
                                        ));
                                    }
                                    Some(compiled)
                                }
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
    match agent_binding.middleware_merge_mode.unwrap_or_default() {
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
                if !equivalent_error_case(existing, &next.schema, inherited, &next.schema) {
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
    let presented_schema = result.schema.clone();
    // A presented command may invoke any next command. Conservatively expose every unknown error
    // at every outward command so callers can handle the complete vocabulary.
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
                if !equivalent_error_case(existing, &presented_schema, inherited, &next.schema) {
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

fn equivalent_error_case(
    left: &ErrorCase,
    left_graph: &SchemaGraph,
    right: &ErrorCase,
    right_graph: &SchemaGraph,
) -> bool {
    left.kind == right.kind
        && left.exit_code == right.exit_code
        && match (&left.payload, &right.payload) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                is_equivalent_cross_graph(left_graph, left, right_graph, right)
            }
            _ => false,
        }
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
        agent_type_name: Some(binding.agent_type_name.clone()),
        tool_name: Some(binding.tool_name.clone()),
        middleware_name,
        message: message.into(),
    }
}

fn global_diagnostic(
    middleware_name: Option<String>,
    message: impl Into<String>,
) -> ToolMiddlewareCompileDiagnostic {
    ToolMiddlewareCompileDiagnostic {
        agent_type_name: None,
        tool_name: None,
        middleware_name,
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "tool_middlewares/tests.rs"]
mod tests;

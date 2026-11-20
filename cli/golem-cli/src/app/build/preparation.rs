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

use super::gen_bridge::{build_owner_components, component_tool_dependency_components};
use super::plan::{BuildEffect, BuildPlanLimits, BuildStage};
use crate::app::context::{BuildContext, ResolvedEnvironmentTools};
use crate::model::app::{ComponentDependency, EnvironmentToolBridgeRequests, SubjectSource};
use crate::model::cascade::property::Property;
use crate::model::cascade::property::tool_bindings::ToolBindingState;
use anyhow::ensure;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::ComponentName;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::tool::ToolName;
use golem_common::model::tool_middleware::ToolMiddlewareName;
use golem_common::model::tool_middleware_release::ToolMiddlewareReleaseReference;
use golem_common::model::tool_release::ToolReleaseReference;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BuildPreparationId {
    Tool(ToolName),
    Middleware(ToolMiddlewareName),
    EnvironmentTools,
}

#[derive(Clone, Debug)]
pub enum BuildPreparationOperation {
    ToolRelease(ToolReleaseReference),
    MiddlewareRelease(ToolMiddlewareReleaseReference),
    EnvironmentTools {
        names: BTreeSet<String>,
        wildcard: bool,
        mcp_imports: Vec<(usize, Option<String>)>,
    },
}

#[derive(Clone, Debug)]
pub struct BuildPreparation {
    pub operation: BuildPreparationOperation,
    pub effects: BTreeSet<BuildEffect>,
}

#[derive(Clone, Debug)]
enum MiddlewareOwner {
    Environment,
    Component(ComponentName),
    Agent(ComponentName, AgentTypeName),
}

#[derive(Clone, Debug)]
struct MiddlewareInput {
    owner: MiddlewareOwner,
    tool: String,
    middleware: BTreeSet<ToolMiddlewareName>,
}

/// Fixed resolver operations and their selection inputs. Metadata selects entries;
/// it cannot add release references, imports, or resolver operations.
#[derive(Clone, Debug, Default)]
pub struct BuildPreparations {
    pub environment: Option<EnvironmentName>,
    pub entries: BTreeMap<BuildPreparationId, BuildPreparation>,
    dependencies: BTreeMap<ComponentName, BTreeSet<ComponentDependency>>,
    manifest_tools: BTreeSet<String>,
    manifest_components: BTreeSet<ComponentName>,
    middleware_inputs: Vec<MiddlewareInput>,
}

impl BuildPreparations {
    pub(super) fn inspect<'a>(
        ctx: &BuildContext<'_>,
        stages: impl Iterator<Item = &'a BuildStage>,
    ) -> anyhow::Result<Self> {
        let app = ctx.application();
        let dependencies = stages
            .map(|stage| {
                (
                    stage.component.clone(),
                    app.component(&stage.component)
                        .properties()
                        .dependencies
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut plan = Self {
            environment: Some(app.environment_name().clone()),
            dependencies,
            manifest_components: ctx.application_context().selected_component_names().clone(),
            ..Self::default()
        };
        if ctx.custom_bridge_sdk_target().is_none() {
            for (_, _, targets) in app.bridge_sdks().for_all_used_modes() {
                if let Some(tools) = &targets.tools {
                    plan.manifest_tools.extend(tools.clone().into_set());
                }
            }
        }
        let release_effects =
            BTreeSet::from([BuildEffect::RemoteRead, BuildEffect::ConditionalGrantUpsert]);
        for (name, reference) in app.remote_release_references() {
            plan.entries.insert(
                BuildPreparationId::Tool(name.clone()),
                BuildPreparation {
                    operation: BuildPreparationOperation::ToolRelease(
                        reference
                            .to_release_reference()
                            .map_err(anyhow::Error::msg)?,
                    ),
                    effects: release_effects.clone(),
                },
            );
        }
        for (name, reference) in app.remote_tool_middleware_release_references() {
            plan.entries.insert(
                BuildPreparationId::Middleware(name.clone()),
                BuildPreparation {
                    operation: BuildPreparationOperation::MiddlewareRelease(
                        reference
                            .to_release_reference()
                            .map_err(anyhow::Error::msg)?,
                    ),
                    effects: release_effects.clone(),
                },
            );
        }
        let scope = app.environment_tool_bridge_requests(
            &plan.dependencies.keys().cloned().collect(),
            ctx.custom_bridge_sdk_target().is_none(),
        );
        if scope.wildcard || !scope.names.is_empty() {
            plan.entries.insert(
                BuildPreparationId::EnvironmentTools,
                BuildPreparation {
                    operation: BuildPreparationOperation::EnvironmentTools {
                        names: scope.names,
                        wildcard: scope.wildcard,
                        mcp_imports: app
                            .mcp_imports(app.environment_name())
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(index, import)| (index, import.import.prefix.clone()))
                            .collect(),
                    },
                    effects: BTreeSet::from([
                        BuildEffect::RemoteRead,
                        BuildEffect::ResolveMcpImports,
                    ]),
                },
            );
        }
        for (tool, binding) in app.environment_tool_bindings() {
            plan.add_binding(MiddlewareOwner::Environment, &tool, &binding)?;
        }
        for component in build_owner_components(ctx) {
            let tool_names = component_tool_dependency_components(ctx, &component)
                .iter()
                .flat_map(|name| app.component(name).properties().dependencies.clone())
                .filter_map(|dependency| match dependency {
                    ComponentDependency::Tool { tool_name, .. } => Some(tool_name.to_string()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            for (tool, binding) in app
                .component(&component)
                .layer_properties()
                .tool_bindings
                .value()
            {
                if tool_names.contains(tool) {
                    plan.add_binding(MiddlewareOwner::Component(component.clone()), tool, binding)?;
                }
            }
            let mapping = app
                .agent_ids()
                .cloned()
                .map(|agent| (agent, component.clone()))
                .collect();
            let agents = app.resolve_agents(&mapping)?;
            for agent in app.agent_ids() {
                for (tool, binding) in agents.agent(agent).expect("declared agent").tool_bindings()
                {
                    if tool_names.contains(tool) {
                        plan.add_binding(
                            MiddlewareOwner::Agent(component.clone(), agent.clone()),
                            tool,
                            binding,
                        )?;
                    }
                }
            }
        }
        Ok(plan)
    }

    fn add_binding(
        &mut self,
        owner: MiddlewareOwner,
        tool: &str,
        binding: &ToolBindingState,
    ) -> anyhow::Result<()> {
        let middleware = binding
            .middleware_installations()
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default()
            .into_iter()
            .map(|installation| installation.name)
            .filter(|name| {
                self.entries
                    .contains_key(&BuildPreparationId::Middleware(name.clone()))
            })
            .collect();
        self.middleware_inputs.push(MiddlewareInput {
            owner,
            tool: tool.into(),
            middleware,
        });
        Ok(())
    }

    pub fn environment_requests(&self) -> Option<EnvironmentToolBridgeRequests> {
        match &self
            .entries
            .get(&BuildPreparationId::EnvironmentTools)?
            .operation
        {
            BuildPreparationOperation::EnvironmentTools {
                names, wildcard, ..
            } => Some(EnvironmentToolBridgeRequests {
                names: names.clone(),
                wildcard: *wildcard,
            }),
            _ => unreachable!(),
        }
    }

    pub fn selected(
        &self,
        components: &BTreeSet<ComponentName>,
        metadata: &BTreeMap<ComponentName, ExtractedComponentMetadata>,
        environment: Option<&ResolvedEnvironmentTools>,
        limits: BuildPlanLimits,
    ) -> anyhow::Result<BTreeSet<BuildPreparationId>> {
        if self.environment.is_none() {
            return Ok(BTreeSet::new());
        }
        let mut selected = BTreeSet::new();
        let mut tools = BTreeSet::new();
        let mut environment_names = BTreeSet::new();
        for component in components {
            let dependencies = self
                .dependencies
                .get(component)
                .ok_or_else(|| anyhow::anyhow!("Unchecked preparation consumer '{component}'"))?;
            for dependency in dependencies {
                if let ComponentDependency::Tool { source, tool_name } = dependency {
                    tools.insert(tool_name.to_string());
                    match source {
                        SubjectSource::RemoteRelease => {
                            selected.insert(BuildPreparationId::Tool(tool_name.clone()));
                        }
                        SubjectSource::EnvironmentTool => {
                            selected.insert(BuildPreparationId::EnvironmentTools);
                            environment_names.insert(tool_name.clone());
                        }
                        SubjectSource::Local { .. } => {}
                    }
                }
            }
        }
        for matcher in &self.manifest_tools {
            for id in self.entries.keys() {
                if let BuildPreparationId::Tool(name) = id
                    && (matcher == "*" || matcher == name.as_str())
                {
                    selected.insert(id.clone());
                    tools.insert(name.to_string());
                }
            }
            for component in &self.manifest_components {
                if let Some(metadata) = metadata.get(component) {
                    for tool in &metadata.tools {
                        if let Some(name) = tool.name()
                            && (matcher == "*" || matcher == component.as_str() || matcher == name)
                        {
                            tools.insert(name.to_string());
                        }
                    }
                }
            }
            if let Some(scope) = self.environment_requests()
                && (matcher == "*" || scope.names.contains(matcher))
            {
                selected.insert(BuildPreparationId::EnvironmentTools);
                if matcher != "*" {
                    tools.insert(matcher.clone());
                    environment_names
                        .insert(ToolName::try_from(matcher.as_str()).map_err(anyhow::Error::msg)?);
                }
            }
        }
        if selected.contains(&BuildPreparationId::EnvironmentTools)
            && let Some(environment) = environment
        {
            self.validate_environment_result(environment, limits)?;
            for name in &environment_names {
                environment.tool(name)?;
            }
            if self.manifest_tools.contains("*") {
                tools.extend(
                    environment
                        .ambient_tools
                        .iter()
                        .map(|tool| tool.name.to_string()),
                );
                tools.extend(
                    environment
                        .mcp_tools
                        .iter()
                        .filter_map(|tool| tool.definition.name().map(str::to_string)),
                );
            }
        }
        ensure!(
            tools.len() <= limits.edges,
            "Build preparation result limit exceeded"
        );
        for input in &self.middleware_inputs {
            if !tools.contains(&input.tool) {
                continue;
            }
            let active = match &input.owner {
                MiddlewareOwner::Environment => true,
                MiddlewareOwner::Component(component) => {
                    components.contains(component)
                        && metadata.get(component).is_none_or(|metadata| {
                            !metadata.tools.is_empty() || metadata.agent_types.is_empty()
                        })
                }
                MiddlewareOwner::Agent(component, agent) => {
                    components.contains(component)
                        && metadata.get(component).is_some_and(|metadata| {
                            metadata
                                .agent_types
                                .iter()
                                .any(|definition| definition.type_name == *agent)
                        })
                }
            };
            if active {
                selected.extend(
                    input
                        .middleware
                        .iter()
                        .cloned()
                        .map(BuildPreparationId::Middleware),
                );
            }
        }
        self.validate_selection(&selected, limits)?;
        Ok(selected)
    }

    pub fn validate_selection(
        &self,
        selected: &BTreeSet<BuildPreparationId>,
        limits: BuildPlanLimits,
    ) -> anyhow::Result<()> {
        ensure!(
            selected.len() <= limits.actions,
            "Build preparation action limit exceeded"
        );
        for id in selected {
            ensure!(
                self.entries.contains_key(id),
                "Unchecked build preparation {id:?}"
            );
        }
        Ok(())
    }

    pub fn validate_environment_result(
        &self,
        result: &ResolvedEnvironmentTools,
        limits: BuildPlanLimits,
    ) -> anyhow::Result<()> {
        let Some(BuildPreparation {
            operation: BuildPreparationOperation::EnvironmentTools { mcp_imports, .. },
            ..
        }) = self.entries.get(&BuildPreparationId::EnvironmentTools)
        else {
            anyhow::bail!("Unchecked environment tool resolver");
        };
        ensure!(
            result
                .ambient_tools
                .len()
                .saturating_add(result.mcp_tools.len())
                .saturating_add(result.mcp_diagnostics.len())
                <= limits.edges,
            "Build preparation result limit exceeded"
        );
        for (index, name, upstream) in result
            .mcp_tools
            .iter()
            .map(|tool| {
                (
                    tool.import_index,
                    tool.definition.name(),
                    tool.upstream_name.as_str(),
                )
            })
            .chain(result.mcp_diagnostics.iter().map(|diagnostic| {
                (
                    diagnostic.import_index,
                    Some(diagnostic.canonical_name.as_str()),
                    diagnostic.upstream_name.as_str(),
                )
            }))
        {
            let (_, prefix) = mcp_imports
                .iter()
                .find(|(candidate, _)| *candidate == index as usize)
                .ok_or_else(|| anyhow::anyhow!("Unchecked MCP import result {index}"))?;
            let sanitized = golem_mcp_import::tool::sanitize_name(upstream);
            let expected = prefix
                .as_ref()
                .map(|prefix| format!("{prefix}-{sanitized}"))
                .unwrap_or(sanitized);
            ensure!(
                name == Some(expected.as_str()),
                "MCP result is outside its checked import scope: {name:?}"
            );
        }
        Ok(())
    }

    pub(super) fn edge_count(&self) -> usize {
        self.dependencies.values().map(BTreeSet::len).sum::<usize>()
            + self.manifest_tools.len()
            + self.manifest_components.len()
            + self
                .middleware_inputs
                .iter()
                .map(|input| 1 + input.middleware.len())
                .sum::<usize>()
            + self
                .environment_requests()
                .map_or(0, |scope| scope.names.len())
            + self
                .entries
                .values()
                .map(|entry| match &entry.operation {
                    BuildPreparationOperation::EnvironmentTools { mcp_imports, .. } => {
                        mcp_imports.len()
                    }
                    _ => 0,
                })
                .sum::<usize>()
    }

    pub(super) fn log(&self) {
        use crate::log::logln;
        for (id, entry) in &self.entries {
            logln(format!(
                "  preparation {id:?} in {:?}: {:?}; effects={:?}",
                self.environment, entry.operation, entry.effects
            ));
        }
        if !self.entries.is_empty() {
            logln(format!(
                "    preparation component inputs: {:?}",
                self.dependencies
            ));
            logln(format!(
                "    preparation bridge tool selectors: {:?}; component metadata scopes: {:?}",
                self.manifest_tools, self.manifest_components
            ));
            for input in &self.middleware_inputs {
                if !input.middleware.is_empty() {
                    logln(format!("    preparation middleware input: {input:?}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::context::ApplicationContext;
    use crate::model::app::{
        Application, ApplicationPreload, BuildConfig, ComponentPresetSelector,
    };
    use crate::model::app_raw;
    use test_r::test;

    fn inventory() -> (BuildPreparations, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("golem.yaml");
        std::fs::write(
            &manifest,
            r#"
manifestVersion: 2.0.0
app: preparation-inputs
environments:
  local:
    server: local
    tools:
      search:
        middleware: [audit]
components:
  app:early:
    componentWasm: early.wasm
    dependencies:
      tools: [search]
    tools:
      search:
        middleware: [owner-audit]
  app:late:
    componentWasm: late.wasm
    dependencies:
      tools: [native-echo, acme-search-files]
tools:
  search:
    release:
      releaseId: 00000000-0000-0000-0000-000000000001
  middleware:
    audit:
      release:
        releaseId: 00000000-0000-0000-0000-000000000002
    owner-audit:
      release:
        releaseId: 00000000-0000-0000-0000-000000000003
mcp:
  imports:
    local:
      - url: https://tools.example/mcp
        prefix: acme
"#,
        )
        .unwrap();
        let raw = vec![app_raw::ApplicationWithSource::from_yaml_file(&manifest).unwrap()];
        let (preload, _, errors) = Application::preload_from_raw_apps(&raw).into_product();
        assert!(errors.is_empty(), "{errors:?}");
        let ApplicationPreload {
            application_name,
            environments,
            local_server,
            ..
        } = preload.unwrap();
        let (app, _, errors) = Application::from_raw_apps(
            directory.path().to_owned(),
            application_name,
            environments,
            local_server,
            ComponentPresetSelector {
                environment: "local".parse().unwrap(),
                presets: vec![],
            },
            raw,
        )
        .into_product();
        assert!(errors.is_empty(), "{errors:?}");
        let ctx = ApplicationContext::for_test(app.unwrap());
        let stages = ctx
            .application()
            .component_names()
            .map(|name| BuildStage {
                component: name.clone(),
                dependencies: BTreeSet::new(),
                inputs: BTreeSet::new(),
                actions: vec![],
            })
            .collect::<Vec<_>>();
        (
            BuildPreparations::inspect(
                &BuildContext::new(&ctx, &BuildConfig::default()),
                stages.iter(),
            )
            .unwrap(),
            directory,
        )
    }

    #[test]
    fn preparation_inputs_select_fixed_releases_and_late_environment_scope() {
        let (plan, _directory) = inventory();
        let limits = BuildPlanLimits::default();
        let early = ComponentName("app:early".into());
        let remote = BuildPreparationId::Tool("search".try_into().unwrap());
        let audit = BuildPreparationId::Middleware("audit".try_into().unwrap());
        let owner_audit = BuildPreparationId::Middleware("owner-audit".try_into().unwrap());
        assert_eq!(plan.entries.len(), 4);
        assert_eq!(
            plan.selected(
                &BTreeSet::from([early.clone()]),
                &BTreeMap::new(),
                None,
                limits
            )
            .unwrap(),
            BTreeSet::from([remote.clone(), audit.clone(), owner_audit.clone()])
        );
        let metadata = BTreeMap::from([(
            early.clone(),
            ExtractedComponentMetadata {
                agent_types: vec![],
                tools: vec![],
                tool_middlewares: vec![],
            },
        )]);
        assert_eq!(
            plan.selected(&BTreeSet::from([early.clone()]), &metadata, None, limits)
                .unwrap(),
            BTreeSet::from([remote.clone(), audit.clone(), owner_audit.clone()])
        );
        assert_eq!(
            plan.selected(
                &BTreeSet::from([early, ComponentName("app:late".into())]),
                &metadata,
                None,
                limits
            )
            .unwrap(),
            BTreeSet::from([
                remote.clone(),
                audit,
                owner_audit,
                BuildPreparationId::EnvironmentTools
            ])
        );
        let requests = plan.environment_requests().unwrap();
        assert!(!requests.wildcard);
        assert_eq!(
            requests.names,
            BTreeSet::from(["native-echo".into(), "acme-search-files".into()])
        );
        assert!(
            plan.entries[&remote]
                .effects
                .contains(&BuildEffect::ConditionalGrantUpsert)
        );
        assert_eq!(
            plan.entries[&BuildPreparationId::EnvironmentTools].effects,
            BTreeSet::from([BuildEffect::RemoteRead, BuildEffect::ResolveMcpImports])
        );
        let definition = |name: &str| golem_common::schema::tool::Tool {
            version: "1.0.0".into(),
            requires_filesystem: false,
            http_mount: None,
            commands: golem_common::schema::tool::CommandTree {
                nodes: vec![golem_common::schema::tool::CommandNode {
                    name: name.into(),
                    aliases: vec![],
                    doc: Default::default(),
                    http: None,
                    globals: Default::default(),
                    subcommands: vec![],
                    body: None,
                }],
            },
            schema: golem_common::schema::SchemaGraph::empty(),
        };
        let mut result = ResolvedEnvironmentTools {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            ambient_tools: vec![
                golem_common::model::deployment::DeploymentPlanAmbientToolEntry {
                    release_id: golem_common::model::tool_release::ToolReleaseId::new(),
                    name: "native-echo".try_into().unwrap(),
                    version: golem_common::model::deployment::ToolVersion("1.0.0".into()),
                    source_digest: Default::default(),
                    owner_account_id: golem_common::model::account::AccountId::new(),
                    owner_account_email: golem_common::model::account::AccountEmail::new(
                        "native@example.com",
                    ),
                    metadata_version: golem_common::model::deployment::ToolMetadataVersion(
                        "1.0.0".into(),
                    ),
                    metadata_digest: Default::default(),
                    definition: definition("native-echo"),
                    provision: Default::default(),
                    environment_binding: Default::default(),
                },
            ],
            mcp_tools: vec![golem_client::model::McpResolvedTool {
                import_index: 0,
                upstream_name: "Search Files".into(),
                digest: "projection".into(),
                definition: definition("acme-search-files"),
            }],
            mcp_diagnostics: vec![],
        };
        let late = BTreeSet::from([ComponentName("app:late".into())]);
        assert_eq!(
            plan.selected(&late, &metadata, Some(&result), limits)
                .unwrap(),
            BTreeSet::from([BuildPreparationId::EnvironmentTools])
        );
        let fixed_edges = plan.edge_count();
        let bounded = super::super::plan::BuildPlan::new(
            vec![],
            BuildPlanLimits {
                edges: fixed_edges + 2,
                ..limits
            },
        )
        .unwrap()
        .with_preparations(plan.clone())
        .unwrap();
        plan.validate_environment_result(&result, bounded.preparation_limits())
            .unwrap();
        assert!(
            plan.validate_environment_result(
                &result,
                BuildPlanLimits {
                    edges: bounded.preparation_limits().edges - 1,
                    ..limits
                }
            )
            .is_err()
        );
        result.mcp_tools[0].import_index = 1;
        assert!(
            plan.selected(&late, &metadata, Some(&result), limits)
                .unwrap_err()
                .to_string()
                .contains("Unchecked MCP import result")
        );
        result.mcp_tools[0].import_index = 0;
        result.mcp_tools[0].upstream_name = "Different Tool".into();
        assert!(
            plan.selected(&late, &metadata, Some(&result), limits)
                .unwrap_err()
                .to_string()
                .contains("outside its checked import scope")
        );
    }

    #[test]
    fn preparation_refuses_unknown_inputs_and_checks_both_sides_of_bounds() {
        let (plan, _directory) = inventory();
        let remote = BuildPreparationId::Tool("search".try_into().unwrap());
        let selected = BTreeSet::from([remote]);
        let mut limits = BuildPlanLimits {
            components: 2,
            actions: 1,
            edges: 2,
        };
        plan.validate_selection(&selected, limits).unwrap();
        limits.actions = 0;
        assert!(
            plan.validate_selection(&selected, limits)
                .unwrap_err()
                .to_string()
                .contains("action limit")
        );
        assert!(
            plan.validate_selection(
                &BTreeSet::from([BuildPreparationId::Tool("undeclared".try_into().unwrap())]),
                BuildPlanLimits::default()
            )
            .unwrap_err()
            .to_string()
            .contains("Unchecked build preparation")
        );
        assert!(
            plan.selected(
                &BTreeSet::from([ComponentName("app:outside".into())]),
                &BTreeMap::new(),
                None,
                BuildPlanLimits::default()
            )
            .unwrap_err()
            .to_string()
            .contains("Unchecked preparation consumer")
        );
        let mut result = ResolvedEnvironmentTools {
            environment_id: golem_common::model::environment::EnvironmentId::new(),
            ambient_tools: vec![],
            mcp_tools: vec![],
            mcp_diagnostics: vec![],
        };
        for index in 0..2 {
            result
                .mcp_diagnostics
                .push(crate::app::context::ResolvedMcpDiagnostic {
                    canonical_name: format!("acme-rejected-{index}"),
                    import_index: 0,
                    upstream_name: format!("rejected-{index}"),
                    reason: "unsupported schema".into(),
                });
        }
        plan.validate_environment_result(&result, limits).unwrap();
        limits.edges = 1;
        assert!(
            plan.validate_environment_result(&result, limits)
                .unwrap_err()
                .to_string()
                .contains("result limit")
        );
        assert!(
            plan.selected(
                &BTreeSet::from([ComponentName("app:late".into())]),
                &BTreeMap::new(),
                Some(&ResolvedEnvironmentTools {
                    mcp_diagnostics: vec![],
                    ..result
                }),
                BuildPlanLimits::default()
            )
            .unwrap_err()
            .to_string()
            .contains("Environment tool dependency 'acme-search-files'")
        );
    }
}

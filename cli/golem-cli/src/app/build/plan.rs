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

use crate::model::app::ComponentDependency;
use crate::model::app_raw::BuildCommand;
use crate::model::language::GuestLanguage;
use anyhow::{bail, ensure};
use golem_common::model::component::ComponentName;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Requirements are observations, not permissions to install or repair anything.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum BuildRequirement {
    HostProgram { program: String, directory: PathBuf },
    Native { backend: NativeBuildBackend },
    ManagedGoToolchain,
    ExistingFile { path: PathBuf },
    Preparation { preparation: CommonDependencies },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "directory")]
pub enum CommonDependencies {
    Npm(PathBuf),
    Moon(PathBuf),
    Go(PathBuf),
}

impl CommonDependencies {
    pub fn manifest(&self) -> PathBuf {
        match self {
            Self::Npm(root) => root.join("package.json"),
            Self::Moon(root) => root.join("moon.mod"),
            Self::Go(root) => root.join("go.mod"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeBuildBackend {
    QuickJsCrate,
    QuickJsTypes,
    QuickJsInjection,
    ComponentMetadata,
    ComponentPreinitialization,
    BridgeGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildEffect {
    ReadFiles,
    WriteFiles,
    RemoveDirectories,
    HostProcess,
    FetchDependencies,
    ExecuteUserComponent,
    RemoteRead,
    ResolveMcpImports,
    ConditionalGrantUpsert,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildAction {
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub requirements: BTreeSet<BuildRequirement>,
    pub effects: BTreeSet<BuildEffect>,
    pub operation: BuildOperation,
}

impl BuildAction {
    pub fn command(directory: &std::path::Path, command: &BuildCommand) -> anyhow::Result<Self> {
        let mut action = Self {
            inputs: Vec::new(),
            outputs: command
                .targets()
                .iter()
                .map(|path| directory.join(path))
                .collect(),
            requirements: BTreeSet::new(),
            effects: BTreeSet::from([BuildEffect::ReadFiles, BuildEffect::WriteFiles]),
            operation: BuildOperation::Command(command.clone()),
        };
        let backend = match command {
            BuildCommand::External(command) => {
                let tokens = shlex::split(&command.command).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Unclassifiable build command: invalid quoting in {:?}",
                        command.command
                    )
                })?;
                let Some(program) = tokens.first().filter(|program| !program.is_empty()) else {
                    bail!("Unclassifiable empty build command");
                };
                let directory = command
                    .dir
                    .as_ref()
                    .map(|dir| directory.join(dir))
                    .unwrap_or_else(|| directory.to_path_buf());
                action.inputs = command
                    .sources
                    .iter()
                    .map(|path| directory.join(path))
                    .collect();
                action.outputs = command
                    .targets
                    .iter()
                    .map(|path| directory.join(path))
                    .collect();
                action.requirements.insert(BuildRequirement::HostProgram {
                    program: program.clone(),
                    directory,
                });
                action.effects.insert(BuildEffect::HostProcess);
                // A host process can fetch dependencies or execute user build scripts.
                // These effects are not a declaration suitable for a confined Tool backend.
                action.effects.insert(BuildEffect::FetchDependencies);
                if !command.rmdirs.is_empty() {
                    action.effects.insert(BuildEffect::RemoveDirectories);
                }
                return Ok(action);
            }
            BuildCommand::QuickJSCrate(command) => {
                action.inputs.push(directory.join(&command.wit));
                action.inputs.extend(
                    command
                        .js_modules
                        .values()
                        .filter(|path| !matches!(path.as_str(), "@composition" | "@slot"))
                        .map(|path| directory.join(path)),
                );
                action.inputs[1..].sort();
                NativeBuildBackend::QuickJsCrate
            }
            BuildCommand::QuickJSDTS(command) => {
                action.inputs.push(directory.join(&command.wit));
                NativeBuildBackend::QuickJsTypes
            }
            BuildCommand::InjectToPrebuiltQuickJs(command) => {
                action.inputs = vec![
                    directory.join(&command.inject_to_prebuilt_quickjs),
                    directory.join(&command.module),
                ];
                NativeBuildBackend::QuickJsInjection
            }
            BuildCommand::PreinitializeJs(command) => {
                action
                    .inputs
                    .push(directory.join(&command.preinitialize_js));
                action.effects.insert(BuildEffect::ExecuteUserComponent);
                NativeBuildBackend::ComponentPreinitialization
            }
        };
        action
            .requirements
            .insert(BuildRequirement::Native { backend });
        Ok(action)
    }

    pub fn metadata(input: PathBuf, output: PathBuf) -> Self {
        Self {
            inputs: vec![input],
            outputs: vec![output],
            requirements: BTreeSet::from([BuildRequirement::Native {
                backend: NativeBuildBackend::ComponentMetadata,
            }]),
            effects: BTreeSet::from([
                BuildEffect::ReadFiles,
                BuildEffect::WriteFiles,
                BuildEffect::ExecuteUserComponent,
            ]),
            operation: BuildOperation::Metadata,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "command")]
pub enum BuildOperation {
    Command(BuildCommand),
    Prebuilt,
    Dependencies(CommonDependencies),
    Metadata,
    Bridge,
    InstalledMetadata {
        package_digest: String,
        layer_digest: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum BuildValue {
    SubjectMetadata {
        dependency: ComponentDependency,
    },
    GuestBridge {
        dependency: ComponentDependency,
        language: GuestLanguage,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStage {
    pub component: ComponentName,
    pub dependencies: BTreeSet<ComponentName>,
    pub inputs: BTreeSet<BuildValue>,
    pub actions: Vec<BuildAction>,
}

/// A known candidate scope selected by a component or one of its future exports.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildSelectionRule {
    pub owner: ComponentName,
    pub after_metadata: bool,
    pub subject: Option<ComponentDependency>,
    pub components: BTreeSet<ComponentName>,
}

impl BuildSelectionRule {
    fn is_active(
        &self,
        selected: &BTreeSet<ComponentName>,
        metadata: &BTreeMap<
            ComponentName,
            golem_common::model::agent::extraction::ExtractedComponentMetadata,
        >,
    ) -> bool {
        selected.contains(&self.owner)
            && (!self.after_metadata || metadata.contains_key(&self.owner))
            && self
                .subject
                .as_ref()
                .is_none_or(|subject| metadata_contains(metadata, subject))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BuildPlanLimits {
    pub components: usize,
    pub actions: usize,
    pub edges: usize,
}

impl Default for BuildPlanLimits {
    fn default() -> Self {
        Self {
            components: 1024,
            actions: 16_384,
            edges: 16_384,
        }
    }
}

/// A bounded, immutable inventory of checked recipes and their inputs.
#[derive(Clone, Debug)]
pub struct BuildPlan {
    stages: BTreeMap<ComponentName, BuildStage>,
    requirements: BTreeSet<BuildRequirement>,
    selected: BTreeSet<ComponentName>,
    selection_rules: Vec<BuildSelectionRule>,
    bridges: BTreeMap<super::BridgeRequestId, BuildAction>,
    output_claims: Vec<super::OutputDirClaim>,
    preparations: super::preparation::BuildPreparations,
    limits: BuildPlanLimits,
}

#[derive(Clone, Default, Debug)]
pub struct BuildProgress {
    pub(super) completed: BTreeSet<ComponentName>,
    pub(super) executed_commands: BTreeMap<ComponentName, usize>,
    pub(super) built_components: BTreeSet<ComponentName>,
    pub(super) validated_targets: Vec<(super::BridgeRequestId, crate::model::app::BridgeSdkTarget)>,
    pub(super) available_guest_bridge_dependencies: BTreeSet<ComponentDependency>,
    pub(super) generated_guest_target_keys: BTreeSet<super::BridgeSdkTargetKey>,
    pub(super) completed_bridge_requests: BTreeSet<super::BridgeRequestId>,
}

impl BuildPlan {
    pub fn new(stages: Vec<BuildStage>, limits: BuildPlanLimits) -> anyhow::Result<Self> {
        ensure!(
            stages.len() <= limits.components,
            "Build plan component limit exceeded"
        );
        let mut indexed = BTreeMap::new();
        for stage in stages {
            let name = stage.component.clone();
            ensure!(
                indexed.insert(name.clone(), stage).is_none(),
                "Duplicate build stage '{name}'"
            );
        }
        let plan = Self {
            selected: indexed.keys().cloned().collect(),
            selection_rules: Vec::new(),
            bridges: BTreeMap::new(),
            output_claims: Vec::new(),
            preparations: super::preparation::BuildPreparations::default(),
            requirements: indexed
                .values()
                .flat_map(|stage| &stage.actions)
                .flat_map(|action| action.requirements.iter().cloned())
                .collect(),
            stages: indexed,
            limits,
        };
        plan.validate(0, 0)?;
        Ok(plan)
    }

    pub fn with_selection(
        mut self,
        selected: BTreeSet<ComponentName>,
        rules: Vec<BuildSelectionRule>,
    ) -> anyhow::Result<Self> {
        let mut edges = self
            .stages
            .values()
            .map(|stage| stage.dependencies.len() + stage.inputs.len())
            .sum::<usize>();
        for name in &selected {
            self.stage(name)?;
        }
        for rule in &rules {
            self.stage(&rule.owner)?;
            edges = edges
                .checked_add(rule.components.len())
                .ok_or_else(|| anyhow::anyhow!("Build plan edge limit exceeded"))?;
            ensure!(edges <= self.limits.edges, "Build plan edge limit exceeded");
            for name in &rule.components {
                self.stage(name)?;
            }
        }
        self.selected = selected;
        self.selection_rules = rules;
        self.validate(0, 0)?;
        Ok(self)
    }

    pub(super) fn with_bridges(
        mut self,
        bridges: BTreeMap<super::BridgeRequestId, BuildAction>,
        claims: Vec<super::OutputDirClaim>,
    ) -> anyhow::Result<Self> {
        self.requirements.extend(
            bridges
                .values()
                .flat_map(|action| action.requirements.iter().cloned()),
        );
        self.bridges = bridges;
        self.output_claims = claims;
        self.validate(0, 0)?;
        Ok(self)
    }

    pub fn explicit_components(&self) -> &BTreeSet<ComponentName> {
        &self.selected
    }

    pub fn required_middleware_components(
        &self,
        metadata: &BTreeMap<
            ComponentName,
            golem_common::model::agent::extraction::ExtractedComponentMetadata,
        >,
    ) -> BTreeSet<ComponentName> {
        let selected = self.selected_components(metadata);
        self.selection_rules
            .iter()
            .filter(|rule| rule.is_active(&selected, metadata))
            .flat_map(|rule| rule.components.iter().cloned())
            .collect()
    }

    pub(super) fn with_preparations(
        mut self,
        preparations: super::preparation::BuildPreparations,
    ) -> anyhow::Result<Self> {
        self.preparations = preparations;
        self.validate(0, 0)?;
        Ok(self)
    }

    pub fn preparation_limits(&self) -> BuildPlanLimits {
        let fixed_edges = self.preparations.edge_count()
            + self
                .selection_rules
                .iter()
                .map(|rule| rule.components.len())
                .sum::<usize>()
            + self
                .stages
                .values()
                .map(|stage| stage.dependencies.len() + stage.inputs.len())
                .sum::<usize>();
        BuildPlanLimits {
            edges: self.limits.edges - fixed_edges,
            ..self.limits
        }
    }

    pub fn preparations(&self) -> &super::preparation::BuildPreparations {
        &self.preparations
    }

    pub(super) fn bridges(&self) -> &BTreeMap<super::BridgeRequestId, BuildAction> {
        &self.bridges
    }

    pub(super) fn output_claims(&self) -> &[super::OutputDirClaim] {
        &self.output_claims
    }

    pub fn selected_components(
        &self,
        metadata: &BTreeMap<
            ComponentName,
            golem_common::model::agent::extraction::ExtractedComponentMetadata,
        >,
    ) -> BTreeSet<ComponentName> {
        selected_components(
            &self.selected,
            &self.selection_rules,
            self.stages(),
            metadata,
        )
    }

    pub fn stages(&self) -> impl Iterator<Item = &BuildStage> {
        self.stages.values()
    }

    pub fn requirements(&self) -> &BTreeSet<BuildRequirement> {
        &self.requirements
    }

    pub fn selection_rules(&self) -> &[BuildSelectionRule] {
        &self.selection_rules
    }

    pub fn stage(&self, component: &ComponentName) -> anyhow::Result<&BuildStage> {
        self.stages
            .get(component)
            .ok_or_else(|| anyhow::anyhow!("Unchecked build stage '{component}'"))
    }

    pub fn metadata_values(
        &self,
        metadata: &BTreeMap<
            ComponentName,
            golem_common::model::agent::extraction::ExtractedComponentMetadata,
        >,
    ) -> BTreeSet<BuildValue> {
        self.stages()
            .flat_map(|stage| &stage.inputs)
            .filter(|value| {
                let BuildValue::SubjectMetadata { dependency } = value else {
                    return false;
                };
                metadata_contains(metadata, dependency)
            })
            .cloned()
            .collect()
    }

    pub fn ready(
        &self,
        progress: &BuildProgress,
        values: &BTreeSet<BuildValue>,
    ) -> Vec<ComponentName> {
        self.stages
            .values()
            .filter(|stage| {
                !progress.completed.contains(&stage.component)
                    && stage.dependencies.is_subset(&progress.completed)
                    && stage.inputs.is_subset(values)
            })
            .map(|stage| stage.component.clone())
            .collect()
    }

    pub fn complete(
        &self,
        progress: &mut BuildProgress,
        component: &ComponentName,
        values: &BTreeSet<BuildValue>,
    ) -> anyhow::Result<()> {
        let stage = self.stage(component)?;
        ensure!(
            stage.dependencies.is_subset(&progress.completed) && stage.inputs.is_subset(values),
            "Build stage '{component}' has incomplete dependencies"
        );
        ensure!(
            !progress.completed.contains(component),
            "Build stage '{component}' already completed"
        );
        progress.completed.insert(component.clone());
        Ok(())
    }

    /// Each generated bridge target consumes an action and a metadata edge in
    /// addition to the fixed inventory and resolved preparation results.
    pub fn validate_bridge_expansion(
        &self,
        targets: usize,
        preparation_edges: usize,
    ) -> anyhow::Result<()> {
        let edges = targets
            .checked_add(preparation_edges)
            .ok_or_else(|| anyhow::anyhow!("Build plan edge limit exceeded"))?;
        self.validate(targets, edges)
    }

    fn validate(&self, additional_actions: usize, additional_edges: usize) -> anyhow::Result<()> {
        ensure!(
            self.stages.len() <= self.limits.components,
            "Build plan component limit exceeded"
        );
        let mut actions = additional_actions
            .checked_add(self.bridges.len())
            .and_then(|actions| actions.checked_add(self.preparations.entries.len()))
            .ok_or_else(|| anyhow::anyhow!("Build plan action limit exceeded"))?;
        ensure!(
            actions <= self.limits.actions,
            "Build plan action limit exceeded"
        );
        let mut edges = additional_edges
            .checked_add(self.preparations.edge_count())
            .and_then(|edges| {
                edges.checked_add(
                    self.selection_rules
                        .iter()
                        .map(|rule| rule.components.len())
                        .sum::<usize>(),
                )
            })
            .ok_or_else(|| anyhow::anyhow!("Build plan edge limit exceeded"))?;
        ensure!(edges <= self.limits.edges, "Build plan edge limit exceeded");
        let mut dependents = BTreeMap::<&ComponentName, Vec<&ComponentName>>::new();
        let mut remaining = BTreeMap::new();
        for stage in self.stages.values() {
            actions = actions
                .checked_add(stage.actions.len())
                .ok_or_else(|| anyhow::anyhow!("Build plan action limit exceeded"))?;
            edges = edges
                .checked_add(stage.dependencies.len() + stage.inputs.len())
                .ok_or_else(|| anyhow::anyhow!("Build plan edge limit exceeded"))?;
            ensure!(
                actions <= self.limits.actions,
                "Build plan action limit exceeded"
            );
            ensure!(edges <= self.limits.edges, "Build plan edge limit exceeded");
            remaining.insert(&stage.component, stage.dependencies.len());
            for dependency in &stage.dependencies {
                ensure!(
                    self.stages.contains_key(dependency),
                    "Build stage '{}' has missing dependency '{dependency}'",
                    stage.component
                );
                dependents
                    .entry(dependency)
                    .or_default()
                    .push(&stage.component);
            }
        }
        let mut ready = remaining
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        let mut visited = 0;
        while let Some(name) = ready.pop_first() {
            visited += 1;
            for dependent in dependents.get(name).into_iter().flatten() {
                let count = remaining.get_mut(dependent).expect("indexed build stage");
                *count -= 1;
                if *count == 0 {
                    ready.insert(*dependent);
                }
            }
        }
        if visited != self.stages.len() {
            bail!(
                "Build dependency cycle: {}",
                remaining
                    .iter()
                    .filter(|(_, count)| **count > 0)
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }
}

pub(super) fn selected_components<'a>(
    selected: &BTreeSet<ComponentName>,
    rules: &[BuildSelectionRule],
    stages: impl Iterator<Item = &'a BuildStage>,
    metadata: &BTreeMap<
        ComponentName,
        golem_common::model::agent::extraction::ExtractedComponentMetadata,
    >,
) -> BTreeSet<ComponentName> {
    let stages = stages
        .map(|stage| (&stage.component, stage))
        .collect::<BTreeMap<_, _>>();
    let mut selected = selected.clone();
    loop {
        let previous = selected.len();
        for rule in rules {
            if rule.is_active(&selected, metadata) {
                selected.extend(rule.components.iter().cloned());
            }
        }
        for name in selected.clone() {
            selected.extend(stages[&name].dependencies.iter().cloned());
        }
        if selected.len() == previous {
            return selected;
        }
    }
}

fn metadata_contains(
    metadata: &BTreeMap<
        ComponentName,
        golem_common::model::agent::extraction::ExtractedComponentMetadata,
    >,
    dependency: &ComponentDependency,
) -> bool {
    let Some(component) = dependency
        .component_name()
        .and_then(|name| metadata.get(name))
    else {
        return false;
    };
    match dependency {
        ComponentDependency::Agent {
            agent_type_name, ..
        } => component
            .agent_types
            .iter()
            .any(|agent| &agent.type_name == agent_type_name),
        ComponentDependency::Tool { tool_name, .. } => component
            .tools
            .iter()
            .any(|tool| tool.name() == Some(tool_name.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn host_action_fixture_identifies_program_inputs_outputs_and_effects() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../test-data/build-plan/host-action.json"
        ))
        .unwrap();
        let command = serde_json::from_value(fixture["operation"]["command"].clone()).unwrap();
        let action = BuildAction::command(std::path::Path::new("/work"), &command).unwrap();
        assert_eq!(serde_json::to_value(action).unwrap(), fixture);
    }

    #[test]
    fn native_action_inputs_do_not_depend_on_module_map_order() {
        for reverse in [false, true] {
            let mut js_modules = std::collections::HashMap::from([
                ("slot".into(), "@slot".into()),
                ("composition".into(), "@composition".into()),
                ("main".into(), "@main.js".into()),
                ("slot-file".into(), "@slot.js".into()),
                ("composition-file".into(), "@composition.js".into()),
            ]);
            let mut entries = (0..16).collect::<Vec<_>>();
            if reverse {
                entries.reverse();
            }
            for index in entries {
                js_modules.insert(format!("module-{index:02}"), format!("{index:02}.js"));
            }
            let action = BuildAction::command(
                std::path::Path::new("/work"),
                &BuildCommand::QuickJSCrate(crate::model::app_raw::GenerateQuickJSCrate {
                    generate_quickjs_crate: "output".into(),
                    wit: "wit".into(),
                    js_modules,
                    world: None,
                }),
            )
            .unwrap();
            assert_eq!(
                action.inputs,
                std::iter::once(PathBuf::from("/work/wit"))
                    .chain((0..16).map(|index| PathBuf::from(format!("/work/{index:02}.js"))))
                    .chain([
                        PathBuf::from("/work/@composition.js"),
                        PathBuf::from("/work/@main.js"),
                        PathBuf::from("/work/@slot.js"),
                    ])
                    .collect::<Vec<_>>()
            );
        }
    }

    fn stage(name: &str, dependencies: &[&str]) -> BuildStage {
        BuildStage {
            component: ComponentName(name.into()),
            dependencies: dependencies
                .iter()
                .map(|name| ComponentName((*name).into()))
                .collect(),
            inputs: BTreeSet::new(),
            actions: Vec::new(),
        }
    }

    fn names(names: &[&str]) -> Vec<ComponentName> {
        names
            .iter()
            .map(|name| ComponentName((*name).into()))
            .collect()
    }

    #[test]
    fn asymmetric_stages_wait_for_all_dependencies_with_independent_progress() {
        let plan = BuildPlan::new(
            vec![
                stage("a-consumer", &["z-provider", "m-other"]),
                stage("z-provider", &[]),
                stage("m-other", &[]),
            ],
            BuildPlanLimits::default(),
        )
        .unwrap();
        let mut progress = BuildProgress::default();
        let values = BTreeSet::new();
        assert_eq!(
            plan.ready(&progress, &values),
            names(&["m-other", "z-provider"])
        );
        assert!(
            plan.complete(&mut progress, &ComponentName("a-consumer".into()), &values)
                .is_err()
        );
        plan.complete(&mut progress, &ComponentName("z-provider".into()), &values)
            .unwrap();
        assert_eq!(plan.ready(&progress, &values), names(&["m-other"]));
        assert!(
            plan.complete(&mut progress, &ComponentName("z-provider".into()), &values)
                .is_err()
        );
        assert_eq!(plan.ready(&progress, &values), names(&["m-other"]));
        plan.complete(&mut progress, &ComponentName("m-other".into()), &values)
            .unwrap();
        assert_eq!(plan.ready(&progress, &values), names(&["a-consumer"]));
        plan.complete(&mut progress, &ComponentName("a-consumer".into()), &values)
            .unwrap();
        assert!(plan.ready(&progress, &values).is_empty());
        assert_eq!(
            plan.ready(&BuildProgress::default(), &values),
            names(&["m-other", "z-provider"])
        );
        assert!(plan.stage(&ComponentName("late".into())).is_err());
    }

    #[test]
    fn graph_limits_are_inclusive() {
        let stages = vec![stage("a", &["b", "c"]), stage("b", &["c"]), stage("c", &[])];
        let limits = BuildPlanLimits {
            components: 3,
            actions: 0,
            edges: 3,
        };
        let plan = BuildPlan::new(stages.clone(), limits).unwrap();
        assert!(
            BuildPlan::new(
                stages.clone(),
                BuildPlanLimits {
                    components: 2,
                    ..limits
                }
            )
            .is_err()
        );
        assert!(BuildPlan::new(stages.clone(), BuildPlanLimits { edges: 2, ..limits }).is_err());
        assert_eq!(
            plan.ready(&BuildProgress::default(), &BTreeSet::new()),
            names(&["c"])
        );
        let mut with_action = stages;
        with_action[0].actions.push(BuildAction {
            inputs: vec![],
            outputs: vec![],
            requirements: BTreeSet::new(),
            effects: BTreeSet::new(),
            operation: BuildOperation::Metadata,
        });
        assert!(BuildPlan::new(with_action.clone(), limits).is_err());
        BuildPlan::new(
            with_action,
            BuildPlanLimits {
                actions: 1,
                ..limits
            },
        )
        .unwrap();
        assert!(
            BuildPlan::new(vec![stage("a", &["missing"])], BuildPlanLimits::default()).is_err()
        );
    }

    #[test]
    fn explicit_selection_does_not_activate_speculative_middleware_artifacts() {
        let rule = BuildSelectionRule {
            owner: ComponentName("owner".into()),
            after_metadata: true,
            subject: None,
            components: names(&["candidate"]).into_iter().collect(),
        };
        let plan = BuildPlan::new(
            vec![stage("owner", &[]), stage("candidate", &[])],
            BuildPlanLimits::default(),
        )
        .unwrap()
        .with_selection(
            names(&["owner", "candidate"]).into_iter().collect(),
            vec![rule.clone()],
        )
        .unwrap();
        assert!(
            plan.required_middleware_components(&BTreeMap::new())
                .is_empty()
        );
        let metadata = BTreeMap::from([(
            ComponentName("owner".into()),
            golem_common::model::agent::extraction::ExtractedComponentMetadata {
                agent_types: Vec::new(),
                tools: Vec::new(),
                tool_middlewares: Vec::new(),
            },
        )]);
        assert_eq!(
            plan.required_middleware_components(&metadata),
            names(&["candidate"]).into_iter().collect()
        );
        let unselected_owner = plan
            .with_selection(names(&["candidate"]).into_iter().collect(), vec![rule])
            .unwrap();
        assert!(
            unselected_owner
                .required_middleware_components(&metadata)
                .is_empty()
        );
    }

    #[test]
    fn bridge_expansion_shares_limits_with_fixed_actions_and_preparation_results() {
        let mut stages = vec![stage("consumer", &["provider"]), stage("provider", &[])];
        stages[1].actions.push(BuildAction {
            inputs: Vec::new(),
            outputs: Vec::new(),
            requirements: BTreeSet::new(),
            effects: BTreeSet::new(),
            operation: BuildOperation::Metadata,
        });
        let limits = BuildPlanLimits {
            components: 2,
            actions: 3,
            edges: 4,
        };
        let plan = BuildPlan::new(stages.clone(), limits).unwrap();
        plan.validate_bridge_expansion(2, 1).unwrap();
        assert!(
            plan.validate_bridge_expansion(3, 0)
                .unwrap_err()
                .to_string()
                .contains("action limit")
        );
        assert!(
            plan.validate_bridge_expansion(2, 2)
                .unwrap_err()
                .to_string()
                .contains("edge limit")
        );
        let tight_edges = BuildPlan::new(stages, BuildPlanLimits { edges: 2, ..limits }).unwrap();
        tight_edges.validate_bridge_expansion(1, 0).unwrap();
        assert!(tight_edges.validate_bridge_expansion(2, 0).is_err());
        assert!(plan.validate_bridge_expansion(usize::MAX, 1).is_err());
    }

    #[test]
    fn future_results_select_only_checked_candidates_without_changing_the_inventory() {
        use golem_common::model::agent::extraction::ExtractedComponentMetadata;

        let roots = names(&["z-owner"]).into_iter().collect();
        let rule = BuildSelectionRule {
            owner: ComponentName("z-owner".into()),
            after_metadata: true,
            subject: None,
            components: names(&["a-candidate"]).into_iter().collect(),
        };
        let stages = vec![
            stage("a-candidate", &["b-provider"]),
            stage("b-provider", &[]),
            stage("z-owner", &[]),
        ];
        assert!(
            BuildPlan::new(
                stages.clone(),
                BuildPlanLimits {
                    components: 3,
                    actions: 0,
                    edges: 1,
                }
            )
            .unwrap()
            .with_selection(
                names(&["z-owner"]).into_iter().collect(),
                vec![rule.clone()]
            )
            .is_err()
        );
        let plan = BuildPlan::new(
            stages.clone(),
            BuildPlanLimits {
                components: 3,
                actions: 0,
                edges: 2,
            },
        )
        .unwrap()
        .with_selection(roots, vec![rule.clone()])
        .unwrap();
        assert_eq!(
            plan.selected_components(&BTreeMap::new()),
            names(&["z-owner"]).into_iter().collect()
        );
        let metadata = BTreeMap::from([(
            ComponentName("z-owner".into()),
            ExtractedComponentMetadata {
                agent_types: vec![],
                tools: vec![],
                tool_middlewares: vec![],
            },
        )]);
        assert_eq!(
            plan.selected_components(&metadata),
            names(&["a-candidate", "b-provider", "z-owner"])
                .into_iter()
                .collect()
        );
        assert_eq!(
            plan.stages()
                .map(|stage| stage.component.clone())
                .collect::<Vec<_>>(),
            names(&["a-candidate", "b-provider", "z-owner"])
        );
        let reversed = BuildPlan::new(
            stages.into_iter().rev().collect(),
            BuildPlanLimits::default(),
        )
        .unwrap()
        .with_selection(
            names(&["z-owner"]).into_iter().collect(),
            vec![rule.clone()],
        )
        .unwrap();
        assert_eq!(
            reversed.selected_components(&metadata),
            plan.selected_components(&metadata)
        );
        assert_eq!(
            reversed.ready(&BuildProgress::default(), &BTreeSet::new()),
            plan.ready(&BuildProgress::default(), &BTreeSet::new())
        );
        assert!(
            BuildPlan::new(vec![stage("z-owner", &[])], BuildPlanLimits::default())
                .unwrap()
                .with_selection(names(&["z-owner"]).into_iter().collect(), vec![rule])
                .is_err()
        );
    }

    #[test]
    fn completed_producer_does_not_fabricate_a_required_bridge_result() {
        let bridge = BuildValue::GuestBridge {
            dependency: ComponentDependency::Agent {
                component_name: ComponentName("z-provider".into()),
                agent_type_name: golem_common::model::agent::AgentTypeName("SpecificAgent".into()),
            },
            language: GuestLanguage::Rust,
        };
        let mut consumer = stage("a-consumer", &["z-provider"]);
        consumer.inputs.insert(bridge.clone());
        let plan = BuildPlan::new(
            vec![consumer, stage("z-provider", &[])],
            BuildPlanLimits::default(),
        )
        .unwrap();
        let mut progress = BuildProgress::default();
        let values = BTreeSet::new();
        plan.complete(&mut progress, &ComponentName("z-provider".into()), &values)
            .unwrap();
        assert!(plan.ready(&progress, &values).is_empty());
        assert!(
            plan.complete(&mut progress, &ComponentName("a-consumer".into()), &values)
                .is_err()
        );
        let values = BTreeSet::from([bridge]);
        assert_eq!(plan.ready(&progress, &values), names(&["a-consumer"]));
        plan.complete(&mut progress, &ComponentName("a-consumer".into()), &values)
            .unwrap();
    }
}

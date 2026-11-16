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

use crate::model::app_raw::BuildCommand;
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
                        .filter(|path| !path.starts_with('@'))
                        .map(|path| directory.join(path)),
                );
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
    Dependencies { program: String, directory: PathBuf },
    Metadata,
    Bridge,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStage {
    pub component: ComponentName,
    pub dependencies: BTreeSet<ComponentName>,
    pub actions: Vec<BuildAction>,
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

/// A bounded graph of checked stages. Metadata may add edges but cannot silently
/// introduce an unchecked program or backend after execution has started.
#[derive(Clone, Debug)]
pub struct BuildPlan {
    stages: BTreeMap<ComponentName, BuildStage>,
    completed: BTreeSet<ComponentName>,
    requirements: BTreeSet<BuildRequirement>,
    limits: BuildPlanLimits,
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
            requirements: indexed
                .values()
                .flat_map(|stage| &stage.actions)
                .flat_map(|action| action.requirements.iter().cloned())
                .collect(),
            stages: indexed,
            completed: BTreeSet::new(),
            limits,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn stages(&self) -> impl Iterator<Item = &BuildStage> {
        self.stages.values()
    }

    pub fn requirements(&self) -> &BTreeSet<BuildRequirement> {
        &self.requirements
    }

    pub fn stage(&self, component: &ComponentName) -> anyhow::Result<&BuildStage> {
        self.stages
            .get(component)
            .ok_or_else(|| anyhow::anyhow!("Unchecked build stage '{component}'"))
    }

    pub fn ready(&self) -> Vec<ComponentName> {
        self.stages
            .values()
            .filter(|stage| {
                !self.completed.contains(&stage.component)
                    && stage.dependencies.is_subset(&self.completed)
            })
            .map(|stage| stage.component.clone())
            .collect()
    }

    pub fn complete(&mut self, component: &ComponentName) -> anyhow::Result<()> {
        let stage = self.stage(component)?;
        ensure!(
            stage.dependencies.is_subset(&self.completed),
            "Build stage '{component}' has incomplete dependencies"
        );
        ensure!(
            !self.completed.contains(component),
            "Build stage '{component}' already completed"
        );
        self.completed.insert(component.clone());
        Ok(())
    }

    /// The candidate replaces only pending stages. Validate the entire graph
    /// before publishing it, including edges to stages already executed.
    pub fn discover(&mut self, stages: Vec<BuildStage>) -> anyhow::Result<()> {
        ensure!(
            stages.len() <= self.limits.components,
            "Build plan component limit exceeded"
        );
        let mut candidate = self.clone();
        for stage in stages {
            for requirement in stage.actions.iter().flat_map(|action| &action.requirements) {
                ensure!(
                    self.requirements.contains(requirement),
                    "Metadata introduced unchecked build requirement {requirement:?} for '{}'; check the complete pipeline before building",
                    stage.component
                );
            }
            if self.completed.contains(&stage.component) {
                ensure!(
                    self.stage(&stage.component)?.dependencies == stage.dependencies,
                    "Metadata changed dependencies of completed build stage '{}'",
                    stage.component
                );
            } else {
                candidate.stages.insert(stage.component.clone(), stage);
            }
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.stages.len() <= self.limits.components,
            "Build plan component limit exceeded"
        );
        let mut actions = 0usize;
        let mut edges = 0usize;
        let mut dependents = BTreeMap::<&ComponentName, Vec<&ComponentName>>::new();
        let mut remaining = BTreeMap::new();
        for stage in self.stages.values() {
            actions = actions
                .checked_add(stage.actions.len())
                .ok_or_else(|| anyhow::anyhow!("Build plan action limit exceeded"))?;
            edges = edges
                .checked_add(stage.dependencies.len())
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

    fn stage(name: &str, dependencies: &[&str]) -> BuildStage {
        BuildStage {
            component: ComponentName(name.into()),
            dependencies: dependencies
                .iter()
                .map(|name| ComponentName((*name).into()))
                .collect(),
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
    fn asymmetric_stages_wait_for_all_dependencies_and_reject_late_cycles() {
        let mut plan = BuildPlan::new(
            vec![
                stage("a-consumer", &["z-provider", "m-other"]),
                stage("z-provider", &[]),
                stage("m-other", &[]),
            ],
            BuildPlanLimits::default(),
        )
        .unwrap();
        assert_eq!(plan.ready(), names(&["m-other", "z-provider"]));
        assert!(plan.complete(&ComponentName("a-consumer".into())).is_err());
        plan.complete(&ComponentName("z-provider".into())).unwrap();
        assert_eq!(plan.ready(), names(&["m-other"]));
        plan.discover(vec![stage("m-other", &["z-provider"])])
            .unwrap();
        assert_eq!(plan.ready(), names(&["m-other"]));
        assert!(
            plan.discover(vec![stage("m-other", &["a-consumer"])])
                .is_err()
        );
        // A rejected discovery leaves the executable frontier unchanged.
        assert_eq!(plan.ready(), names(&["m-other"]));
        plan.complete(&ComponentName("m-other".into())).unwrap();
        assert_eq!(plan.ready(), names(&["a-consumer"]));
        assert!(
            plan.discover(vec![stage("z-provider", &["a-consumer"])])
                .is_err()
        );
        plan.complete(&ComponentName("a-consumer".into())).unwrap();
        assert!(plan.ready().is_empty());
    }

    #[test]
    fn late_metadata_edges_reorder_pending_stages_without_new_requirements() {
        let mut plan = BuildPlan::new(
            vec![stage("a", &[]), stage("b", &[]), stage("z", &[])],
            BuildPlanLimits::default(),
        )
        .unwrap();
        plan.complete(&ComponentName("b".into())).unwrap();
        plan.discover(vec![stage("a", &["z"])]).unwrap();
        assert_eq!(plan.ready(), names(&["z"]));
        let mut unsupported = stage("late", &[]);
        unsupported.actions.push(BuildAction {
            inputs: vec![],
            outputs: vec![],
            effects: BTreeSet::from([BuildEffect::HostProcess]),
            requirements: BTreeSet::from([BuildRequirement::HostProgram {
                program: "missing".into(),
                directory: "/work".into(),
            }]),
            operation: BuildOperation::Metadata,
        });
        assert!(
            plan.discover(vec![unsupported])
                .unwrap_err()
                .to_string()
                .contains("unchecked")
        );
        assert!(plan.stage(&ComponentName("late".into())).is_err());
        plan.complete(&ComponentName("z".into())).unwrap();
        assert_eq!(plan.ready(), names(&["a"]));
    }

    #[test]
    fn graph_limits_are_inclusive_and_apply_to_discovery() {
        let stages = vec![stage("a", &["b", "c"]), stage("b", &["c"]), stage("c", &[])];
        let limits = BuildPlanLimits {
            components: 3,
            actions: 0,
            edges: 3,
        };
        let mut plan = BuildPlan::new(stages.clone(), limits).unwrap();
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
        assert!(plan.discover(vec![stage("d", &[])]).is_err());
        assert_eq!(plan.ready(), names(&["c"]));
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
}

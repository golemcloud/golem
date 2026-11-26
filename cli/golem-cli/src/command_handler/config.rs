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

use crate::app::build::extract_component_metadata::extract_and_store_agent_types;
use crate::app::context::BuildContext;
use crate::command::config::ConfigSubcommand;
use crate::command_handler::Handlers;
use crate::command_handler::template::EnvVarRenderer;
use crate::context::Context;
use crate::model::app::{Application, BuildConfig};
use crate::model::config::resolver::{ConfigLayer, ConfigSchemaResolution, ConfigSourcePlan};
use crate::model::config::{
    ConfigGetView, ConfigListView, StaticConfigFieldView, StaticConfigSource,
};
use anyhow::{Context as _, bail};
use golem_common::model::agent::{AgentConfigSource, AgentTypeName};
use golem_common::model::component::ComponentName;
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, typed_schema_value_with_projected_defs,
};
use golem_common::schema::config::{ConfigError, ConfigFieldSchema, ConfigOrigin, ConfigSchema};
use golem_common::schema::render::to_json_value;
use golem_common::schema::{AgentTypeSchema, SchemaGraph};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ConfigCommandHandler {
    ctx: Arc<Context>,
}

impl ConfigCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ConfigSubcommand) -> anyhow::Result<()> {
        match command {
            ConfigSubcommand::List {
                component,
                agent_type,
            } => self.cmd_list(component, agent_type).await,
            ConfigSubcommand::Get { field, component } => self.cmd_get(field, component).await,
        }
    }

    async fn cmd_list(
        &self,
        component: Option<ComponentName>,
        agent_type: Option<AgentTypeName>,
    ) -> anyhow::Result<()> {
        let fields = self.prepare_fields(component, agent_type, false).await?;
        self.ctx
            .log_handler()
            .log_output(ConfigListView { fields })?;
        Ok(())
    }

    async fn cmd_get(&self, field: String, component: Option<ComponentName>) -> anyhow::Result<()> {
        let (agent_type, path) = if component.is_some() {
            (None, crate::args::parse_agent_config_path(&field)?)
        } else {
            let Some((owner, path)) = field.split_once('.') else {
                bail!("Use an exact agent-type.field path, or --component NAME FIELD");
            };
            (
                Some(AgentTypeName(owner.into())),
                crate::args::parse_agent_config_path(path)?,
            )
        };
        let component_only = agent_type.is_none();
        let fields = self
            .prepare_fields(component, agent_type, component_only)
            .await?;
        let mut matches = fields.into_iter().filter(|field| field.path == path);
        let Some(field) = matches.next() else {
            bail!("No static config declaration at '{}'", path.join("."));
        };
        if matches.next().is_some() {
            bail!("Config path is ambiguous; select its component explicitly");
        }
        self.ctx.log_handler().log_output(ConfigGetView { field })?;
        Ok(())
    }

    async fn prepare_fields(
        &self,
        component_name: Option<ComponentName>,
        agent_type_name: Option<AgentTypeName>,
        component_only: bool,
    ) -> anyhow::Result<Vec<StaticConfigFieldView>> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let app_ctx = app_ctx.some_or_err_mut()?;
        let components = app_ctx
            .application()
            .component_names()
            .filter(|name| {
                component_name
                    .as_ref()
                    .is_none_or(|selected| selected == *name)
            })
            .cloned()
            .collect::<Vec<_>>();
        if components.is_empty() {
            bail!("No matching component in the application");
        }
        let mut fields = Vec::new();
        let mut owner_found = agent_type_name.is_none();
        for name in &components {
            let agent_types = extract_and_store_agent_types(
                &BuildContext::new(app_ctx, &BuildConfig::new()),
                name,
            )
            .await?;
            owner_found |= agent_types
                .iter()
                .any(|agent| Some(&agent.type_name) == agent_type_name.as_ref());
            fields.extend(prepare_static_config(
                app_ctx.application(),
                name,
                &agent_types,
                |owner| match agent_type_name.as_ref() {
                    Some(selected) => owner == Some(selected),
                    None => !component_only || owner.is_none(),
                },
            )?);
        }
        if !owner_found {
            bail!("Agent type '{}' is not declared", agent_type_name.unwrap());
        }
        Ok(fields)
    }
}

/// Shared by inspection and staging so a displayed fill is the value sent to the registry.
pub(crate) fn prepare_static_config(
    app: &Application,
    component_name: &ComponentName,
    agent_types: &[AgentTypeSchema],
    include_owner: impl Fn(Option<&AgentTypeName>) -> bool,
) -> anyhow::Result<Vec<StaticConfigFieldView>> {
    let component = app.component(component_name);
    let renderer = EnvVarRenderer::new();
    let empty_catalog = Default::default();
    let catalog = app.config_catalog().unwrap_or(&empty_catalog);
    let mut fields = Vec::new();
    let mut resolve = |agent_type: Option<&AgentTypeName>,
                       graph: &SchemaGraph,
                       declarations: &[AgentConfigDeclarationSchema],
                       layers: &[ConfigLayer]|
     -> anyhow::Result<()> {
        if !include_owner(agent_type) {
            return Ok(());
        }
        let code_origin = match agent_type {
            Some(_) => ConfigOrigin {
                file: component.wasm(),
                path: vec!["config".into()],
                layer: "code".into(),
            },
            None => component
                .layer_properties()
                .config_schema_origin
                .clone()
                .unwrap_or_else(|| ConfigOrigin {
                    file: component.source().to_path_buf(),
                    path: vec!["configSchema".into()],
                    layer: "code".into(),
                }),
        };
        let mut schema = ConfigSchema::from_declarations(graph, declarations, &code_origin)?;
        schema.fields.retain(|field| {
            declarations.iter().any(|declaration| {
                declaration.path == field.path && declaration.source == AgentConfigSource::Local
            })
        });
        if schema.fields.is_empty() {
            return Ok(());
        }
        let layers = layers
            .iter()
            .map(|layer| ConfigLayer {
                origin: layer.origin.clone(),
                value: project_owner_layer(&layer.value, &[], &schema.fields),
            })
            .collect::<Vec<_>>();
        let plans = schema.resolve_with(catalog, &layers, &code_origin, &|value, origin| {
            renderer
                .render_json_value(value)
                .map_err(|error| ConfigError {
                    origin: origin.clone(),
                    message: format!("cannot evaluate static config: {error}"),
                })
        })?;
        for (declaration, plan) in schema.fields.iter().zip(plans) {
            let ConfigSourcePlan::Static(value) = plan.source else {
                bail!("Static data config cannot materialize managed descriptors");
            };
            let fill = to_json_value(&schema.graph, &declaration.value_type, &value)?;
            let value = typed_schema_value_with_projected_defs(
                &schema.graph,
                declaration.value_type.clone(),
                value,
            );
            fields.push(StaticConfigFieldView {
                component: component_name.clone(),
                agent_type: agent_type.cloned(),
                path: plan.path,
                schema: value.graph().clone(),
                fill: fill.into(),
                sources: plan
                    .origins
                    .into_iter()
                    .map(|(path, origin)| StaticConfigSource { path, origin })
                    .collect(),
                catalog_origin: plan.catalog_origin,
            });
        }
        Ok(())
    };
    resolve(
        None,
        &component.config_schema().schema,
        &component.config_schema().declarations,
        &component.layer_properties().config_layers,
    )
    .with_context(|| format!("Invalid static config for component {component_name}"))?;
    let mapping = agent_types
        .iter()
        .map(|agent| (agent.type_name.clone(), component_name.clone()))
        .collect::<BTreeMap<_, _>>();
    let agents = app.resolve_agents(&mapping)?;
    for agent in agent_types {
        resolve(
            Some(&agent.type_name),
            &agent.schema,
            &agent.config,
            agents.agent(&agent.type_name).unwrap().config_layers(),
        )
        .with_context(|| format!("Invalid static config for agent {}", agent.type_name))?;
    }
    Ok(fields)
}

// Component/template fills are shared by several agent schemas. Project literal
// paths, but leave selectors and malformed groups intact for schema validation.
fn project_owner_layer(value: &Value, path: &[String], fields: &[ConfigFieldSchema]) -> Value {
    if fields.iter().any(|field| field.path == path) || value.get("$config").is_some() {
        return value.clone();
    }
    match value.as_object() {
        Some(object) => Value::Object(
            object
                .iter()
                .filter_map(|(name, value)| {
                    let mut child = path.to_vec();
                    child.push(name.clone());
                    fields
                        .iter()
                        .any(|field| field.path.starts_with(&child))
                        .then(|| (name.clone(), project_owner_layer(value, &child, fields)))
                })
                .collect(),
        ),
        None => value.clone(),
    }
}

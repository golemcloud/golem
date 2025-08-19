use crate::model::app_raw;
use minijinja::{Environment, Error};
use serde::Serialize;
use std::collections::HashMap;

pub trait Template<C: Serialize> {
    type Rendered;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error>;
}

impl<C: Serialize> Template<C> for String {
    type Rendered = String;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        env.render_str(self, ctx)
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for Option<T> {
    type Rendered = Option<T::Rendered>;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        match self {
            Some(template) => Ok(Some(template.render(env, ctx)?)),
            None => Ok(None),
        }
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for Vec<T> {
    type Rendered = Vec<T::Rendered>;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        self.iter().map(|elem| elem.render(env, ctx)).collect()
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for HashMap<String, T> {
    type Rendered = HashMap<String, T::Rendered>;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        let mut rendered = HashMap::<String, T::Rendered>::with_capacity(self.len());
        for (key, template) in self {
            rendered.insert(key.clone(), template.render(env, ctx)?);
        }
        Ok(rendered)
    }
}

impl<C: Serialize> Template<C> for app_raw::BuildCommand {
    type Rendered = app_raw::BuildCommand;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        match self {
            app_raw::BuildCommand::External(external) => {
                Ok(app_raw::BuildCommand::External(external.render(env, ctx)?))
            }
            app_raw::BuildCommand::QuickJSCrate(generate_quickjs_crate) => Ok(
                app_raw::BuildCommand::QuickJSCrate(generate_quickjs_crate.render(env, ctx)?),
            ),
            app_raw::BuildCommand::QuickJSDTS(generate_quickjs_dts) => Ok(
                app_raw::BuildCommand::QuickJSDTS(generate_quickjs_dts.render(env, ctx)?),
            ),
            app_raw::BuildCommand::AgentWrapper(agent_wrapper) => Ok(
                app_raw::BuildCommand::AgentWrapper(agent_wrapper.render(env, ctx)?),
            ),
            app_raw::BuildCommand::ComposeAgentWrapper(compose_agent_wrapper) => Ok(
                app_raw::BuildCommand::ComposeAgentWrapper(compose_agent_wrapper.render(env, ctx)?),
            ),
            app_raw::BuildCommand::InjectToPrebuiltQuickJs(inject_to_prebuilt_quickjs) => {
                Ok(app_raw::BuildCommand::InjectToPrebuiltQuickJs(
                    inject_to_prebuilt_quickjs.render(env, ctx)?,
                ))
            }
        }
    }
}

impl<C: Serialize> Template<C> for app_raw::ExternalCommand {
    type Rendered = app_raw::ExternalCommand;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        Ok(app_raw::ExternalCommand {
            command: self.command.render(env, ctx)?,
            dir: self.dir.render(env, ctx)?,
            rmdirs: self.rmdirs.render(env, ctx)?,
            mkdirs: self.mkdirs.render(env, ctx)?,
            sources: self.sources.render(env, ctx)?,
            targets: self.targets.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::GenerateQuickJSCrate {
    type Rendered = app_raw::GenerateQuickJSCrate;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        Ok(app_raw::GenerateQuickJSCrate {
            generate_quickjs_crate: self.generate_quickjs_crate.render(env, ctx)?,
            wit: self.wit.render(env, ctx)?,
            js_modules: HashMap::from_iter(
                self.js_modules
                    .iter()
                    .map(|(k, v)| {
                        k.render(env, ctx)
                            .and_then(|k| v.render(env, ctx).map(|v| (k, v)))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            world: self.world.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::GenerateQuickJSDTS {
    type Rendered = app_raw::GenerateQuickJSDTS;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        Ok(app_raw::GenerateQuickJSDTS {
            generate_quickjs_dts: self.generate_quickjs_dts.render(env, ctx)?,
            wit: self.wit.render(env, ctx)?,
            world: self.world.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::GenerateAgentWrapper {
    type Rendered = app_raw::GenerateAgentWrapper;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        Ok(app_raw::GenerateAgentWrapper {
            generate_agent_wrapper: self.generate_agent_wrapper.render(env, ctx)?,
            based_on_compiled_wasm: self.based_on_compiled_wasm.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::ComposeAgentWrapper {
    type Rendered = app_raw::ComposeAgentWrapper;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        Ok(app_raw::ComposeAgentWrapper {
            compose_agent_wrapper: self.compose_agent_wrapper.render(env, ctx)?,
            with_agent: self.with_agent.render(env, ctx)?,
            to: self.to.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::InjectToPrebuiltQuickJs {
    type Rendered = app_raw::InjectToPrebuiltQuickJs;

    fn render(&self, env: &Environment, ctx: &C) -> Result<Self::Rendered, Error> {
        Ok(app_raw::InjectToPrebuiltQuickJs {
            inject_to_prebuilt_quickjs: self.inject_to_prebuilt_quickjs.render(env, ctx)?,
            module: self.module.render(env, ctx)?,
            module_wasm: self.module_wasm.render(env, ctx)?,
            into: self.into.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for serde_json::Value {
    type Rendered = serde_json::Value;

    #[allow(clippy::only_used_in_recursion)]
    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        Ok(match self {
            value @ serde_json::Value::Null => value.clone(),
            value @ serde_json::Value::Bool(_) => value.clone(),
            value @ serde_json::Value::Number(_) => value.clone(),
            value @ serde_json::Value::String(_) => value.clone(),
            serde_json::Value::Array(elems) => {
                let mut rendered_elems = Vec::<serde_json::Value>::with_capacity(elems.len());
                for template in elems {
                    rendered_elems.push(template.render(env, ctx)?);
                }
                serde_json::Value::Array(rendered_elems)
            }
            serde_json::Value::Object(props) => {
                let mut rendered_props =
                    serde_json::Map::<String, serde_json::Value>::with_capacity(props.len());
                for (name, template) in props {
                    rendered_props.insert(name.clone(), template.render(env, ctx)?);
                }
                serde_json::Value::Object(rendered_props)
            }
        })
    }
}

impl<C: Serialize> Template<C> for serde_json::Map<String, serde_json::Value> {
    type Rendered = serde_json::Map<String, serde_json::Value>;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        let mut rendered = serde_json::Map::<String, serde_json::Value>::with_capacity(self.len());
        for (key, template) in self {
            rendered.insert(key.clone(), template.render(env, ctx)?);
        }
        Ok(rendered)
    }
}

impl<C: Serialize> Template<C> for app_raw::ComponentProperties {
    type Rendered = app_raw::ComponentProperties;

    fn render(
        &self,
        env: &minijinja::Environment,
        ctx: &C,
    ) -> Result<Self::Rendered, minijinja::Error> {
        Ok(app_raw::ComponentProperties {
            source_wit: self.source_wit.render(env, ctx)?,
            generated_wit: self.generated_wit.render(env, ctx)?,
            component_wasm: self.component_wasm.render(env, ctx)?,
            linked_wasm: self.linked_wasm.render(env, ctx)?,
            build: self.build.render(env, ctx)?,
            custom_commands: self.custom_commands.render(env, ctx)?,
            clean: self.clean.render(env, ctx)?,
            component_type: self.component_type,
            files: self.files.clone(),
            plugins: self.plugins.clone(),
            env: self.env.render(env, ctx)?,
        })
    }
}

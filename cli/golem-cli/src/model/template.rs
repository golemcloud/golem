use crate::model::app_raw;
use indexmap::IndexMap;
use minijinja::{Environment, Error};
use serde::Serialize;
use std::collections::HashMap;

pub trait Template<C>
where
    C: Serialize,
    Self: Sized,
{
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error>;

    fn render_or_clone(&self, env: &Environment, ctx: Option<&C>) -> Result<Self, Error>
    where
        Self: Clone,
    {
        match ctx {
            Some(ctx) => self.render(env, ctx),
            None => Ok(self.clone()),
        }
    }
}

impl<C: Serialize> Template<C> for String {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        env.render_str(self, ctx)
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for Option<T> {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        match self {
            Some(template) => Ok(Some(template.render(env, ctx)?)),
            None => Ok(None),
        }
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for Vec<T> {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        self.iter().map(|elem| elem.render(env, ctx)).collect()
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for HashMap<String, T> {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        let mut rendered = HashMap::<String, T>::with_capacity(self.len());
        for (key, template) in self {
            rendered.insert(key.clone(), template.render(env, ctx)?);
        }
        Ok(rendered)
    }
}

impl<C: Serialize, T: Template<C>> Template<C> for IndexMap<String, T> {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        let mut rendered = IndexMap::<String, T>::with_capacity(self.len());
        for (key, template) in self {
            rendered.insert(key.clone(), template.render(env, ctx)?);
        }
        Ok(rendered)
    }
}

impl<C: Serialize> Template<C> for app_raw::BuildCommand {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
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
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
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
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
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
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        Ok(app_raw::GenerateQuickJSDTS {
            generate_quickjs_dts: self.generate_quickjs_dts.render(env, ctx)?,
            wit: self.wit.render(env, ctx)?,
            world: self.world.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::GenerateAgentWrapper {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        Ok(app_raw::GenerateAgentWrapper {
            generate_agent_wrapper: self.generate_agent_wrapper.render(env, ctx)?,
            based_on_compiled_wasm: self.based_on_compiled_wasm.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::ComposeAgentWrapper {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        Ok(app_raw::ComposeAgentWrapper {
            compose_agent_wrapper: self.compose_agent_wrapper.render(env, ctx)?,
            with_agent: self.with_agent.render(env, ctx)?,
            to: self.to.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for app_raw::InjectToPrebuiltQuickJs {
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        Ok(app_raw::InjectToPrebuiltQuickJs {
            inject_to_prebuilt_quickjs: self.inject_to_prebuilt_quickjs.render(env, ctx)?,
            module: self.module.render(env, ctx)?,
            module_wasm: self.module_wasm.render(env, ctx)?,
            into: self.into.render(env, ctx)?,
        })
    }
}

impl<C: Serialize> Template<C> for serde_json::Value {
    #[allow(clippy::only_used_in_recursion)]
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
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
    fn render(&self, env: &Environment, ctx: &C) -> Result<Self, Error> {
        let mut rendered = serde_json::Map::<String, serde_json::Value>::with_capacity(self.len());
        for (key, template) in self {
            rendered.insert(key.clone(), template.render(env, ctx)?);
        }
        Ok(rendered)
    }
}

use crate::model::app_raw;
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
            extensions: self.extensions.render(env, ctx)?,
        })
    }
}

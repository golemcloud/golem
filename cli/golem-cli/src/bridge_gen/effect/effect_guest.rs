// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::{effect_result_alias, effect_stream_alias, effect_type_reference};
use crate::bridge_gen::BridgeGenerator;
use crate::bridge_gen::parameter_naming::ParameterNaming;
use crate::bridge_gen::typescript::ts_writer::TsWriter;
use crate::bridge_gen::typescript::*;
use crate::sdk_overrides::sdk_overrides;
use camino::Utf8Path;
use golem_common::model::agent::AgentMode;
use golem_common::schema::agent::AgentTypeSchema;
use serde_json::json;

pub struct EffectGuestBridgeGenerator {
    inner: TypeScriptBridgeGenerator,
}

impl BridgeGenerator for EffectGuestBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        let stream_alias = effect_stream_alias(&agent_type.type_name.0);
        let result_alias = effect_result_alias(&agent_type.type_name.0);
        let mut inner = TypeScriptBridgeGenerator::new_guest_with_extra_reserved_names(
            agent_type,
            target_path,
            testing,
            vec!["Effect".into(), result_alias.clone(), stream_alias],
        )?;
        inner.set_guest_result_repr(GuestResultRepr::Effect(result_alias));
        Ok(Self { inner })
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        let inner = &self.inner;
        std::fs::create_dir_all(&inner.target_path)?;
        let library = inner.library_name();
        let mut body = TsWriter::new();
        inner.generate_ts_type_definitions(&mut body)?;
        self.class(&mut body)?;
        let mut writer = TsWriter::new();
        let stream_alias = effect_stream_alias(&inner.agent_type.type_name.0);
        let result_alias = effect_result_alias(&inner.agent_type.type_name.0);
        writer.write_line(format!(
            "import {{ Effect, Result as {result_alias}, Stream as {stream_alias} }} from 'effect';"
        ));
        writer.write_line("import { Bridge as base } from '@golemcloud/effect-golem';");
        writer.write_line(inner.schema_graphs.borrow().definitions());
        writer.write_line(effect_type_reference(&body.finish_string(), &stream_alias));
        writer.finish(&inner.target_path.join(format!("{library}.ts")))?;
        let dependency = sdk_overrides()?.effect_golem_dep()?;
        let package = json!({
            "name": library, "version": "0.0.1", "type": "module",
            "main": format!("{library}.js"), "types": format!("{library}.d.ts"),
            "scripts": { "build": "tsc" },
            "dependencies": { "@golemcloud/effect-golem": dependency, "effect": crate::versions::effect_dep::EFFECT },
            "devDependencies": { "typescript": "^5.9", "@types/node": "^25" }
        });
        std::fs::write(
            inner.target_path.join("package.json"),
            serde_json::to_string_pretty(&package)?,
        )?;
        let config = json!({ "compilerOptions": {
            "target": "es2020", "module": "esnext", "moduleResolution": "bundler",
            "strict": true, "declaration": true, "skipLibCheck": true
        }, "include": [format!("{library}.ts")] });
        std::fs::write(
            inner.target_path.join("tsconfig.json"),
            serde_json::to_string_pretty(&config)?,
        )?;
        Ok(())
    }
}

impl EffectGuestBridgeGenerator {
    fn class(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        let inner = &self.inner;
        let class = &inner.agent_type.type_name.0;
        writer.begin_export_class(class);
        writer.write_line(
            "private constructor(private readonly resolved: base.RemoteAgentHandle, private readonly scopeCard?: base.ScopeCard) {}",
        );
        let durable = inner.agent_type.mode == AgentMode::Durable;
        if durable {
            writer.write_line(
                "/** The ID of the agent, read from the host. On a client of `upsert` it creates the agent when it is missing. */\nget agentId(): base.RemoteAgentHandle['agentId'] { return this.resolved.agentId; }",
            );
        }
        writer.write_line(format!(
            r#"
/** A client for the same agent whose awaited calls borrow `scopeCard` by default; `undefined` sends none. Triggers and schedules never send a scope card. A scope card is valid only during the invocation that derived it. Triggers and schedules succeed with a scoped invocation handle once the target has durably accepted the invocation; its ID only locates the invocation and is not authority, closing its scope or interrupting its await only detaches, and cancelling a running invocation fails with `RpcCannotCancelRunning`. */
withScopeCard(scopeCard: base.ScopeCard | undefined): {class} {{ return new {class}(this.resolved, scopeCard); }}
"#
        ));
        self.upsert(writer)?;
        if durable {
            self.create(writer)?;
            writer.write_line(format!(
                "/** Connects to the existing agent with the ID. It never creates the agent: an unknown ID, or the ID of an agent of another type, fails with a `not-found` cause. */\nstatic getById(agentId: Parameters<typeof base.getRemoteAgentById>[1]): Effect.Effect<{class}, base.RemoteCallError, base.ConnectionRequirements> {{ return Effect.map(base.getRemoteAgentById({}, agentId), (resolved) => new {class}(resolved)); }}",
                serde_json::to_string(class)?
            ));
        }
        let mut naming = ParameterNaming::new();
        naming.reserve_many([
            "resolved",
            "constructor",
            "agentId",
            "scopeCard",
            "withScopeCard",
        ]);
        for method in &inner.agent_type.methods {
            let member = naming.fresh(inner.to_js_ident(&method.name));
            let stream_alias = effect_stream_alias(&inner.agent_type.type_name.0);
            let args =
                effect_type_reference(&inner.input_type_list(&method.input_schema)?, &stream_alias);
            let encode = inner.build_encode_args_fn(method)?;
            let decode = inner.build_guest_decode_result_fn(method)?;
            let name = serde_json::to_string(&method.name)?;
            let await_name = if durable {
                "invokeAndAwait"
            } else {
                "invokeAndAwaitWithMetadata"
            };
            writer.write_line(format!(
                r#"
readonly {member} = (() => {{
  const __encode = {encode};
  const __decode = {decode};
  const __await = (scopeCard: base.ScopeCard | undefined, ...args: [{args}]) => base.attempt(() => __encode(args)).pipe(
    Effect.flatMap(input => this.resolved.{await_name}({name}, input, __decode, {{ scopeCard }})));
  const __call = (...args: [{args}]) => __await(this.scopeCard, ...args);
  const withScopeCard = (scopeCard: base.ScopeCard | undefined) => (...args: [{args}]) => __await(scopeCard, ...args);
"#
            ));
            if method.uses_streams(&inner.agent_type.schema) {
                writer.write_line("return Object.assign(__call, { withScopeCard }); })();");
                continue;
            }
            writer.write_line(format!(
                r#"
  return Object.assign(__call, {{
    withScopeCard,
    trigger: (...args: [{args}]) => base.attempt(() => __encode(args)).pipe(
      Effect.flatMap(input => this.resolved.trigger({name}, input, __decode))),
    schedule: (at: Parameters<base.RemoteAgentHandle['schedule']>[0], ...args: [{args}]) =>
      base.attempt(() => __encode(args)).pipe(
        Effect.flatMap(input => this.resolved.schedule(at, {name}, input, __decode))),
  }});
}})();
"#
            ));
        }
        writer.end_export_class();
        Ok(())
    }

    fn upsert(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        let inner = &self.inner;
        let class = &inner.agent_type.type_name.0;
        let mut naming = inner.constructor_parameter_naming()?;
        let payload = naming.fresh("constructorPayload");
        writer.write_doc(match inner.agent_type.mode {
            AgentMode::Durable => {
                "Connects to the keyed agent of the constructor parameters, and creates it when it is missing."
            }
            AgentMode::Ephemeral => {
                "Connects to the agent type; each invocation runs on a new one-shot instance."
            }
        });
        let mut method = writer.begin_static_method("upsert");
        inner.write_parameter_list(&mut method, &inner.agent_type.constructor.input_schema)?;
        method.result(&format!(
            "Effect.Effect<{class}, base.RemoteCallError, base.ConnectionRequirements>"
        ));
        method.write_line("return Effect.gen(function* () {");
        method.write_line(format!("const {payload} = yield* base.attempt(() => {{"));
        method.write_line(format!("const {payload}: base.SchemaValue = "));
        inner.write_encode_input_record(
            &mut method,
            &inner.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        method.write_line(format!("return {payload}; }});"));
        method.write_line(format!(
            "return new {class}(yield* base.upsertRemoteAgent({}, {payload}));",
            serde_json::to_string(class)?
        ));
        method.write_line("});");
        Ok(())
    }

    fn create(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        let inner = &self.inner;
        let class = &inner.agent_type.type_name.0;
        let configs = inner.local_configs();
        let mut naming = inner.constructor_parameter_naming()?;
        let options = naming.fresh("options");
        let config_names: Vec<_> = configs
            .iter()
            .map(|c| naming.fresh(TypeScriptBridgeGenerator::guest_config_parameter_name(c)))
            .collect();
        let payload = naming.fresh("constructorPayload");
        let config = naming.fresh("agentConfig");
        writer.write_doc(
            "Creates a durable agent, and connects to it after its initialization. With `keyed`, it creates the keyed agent of the constructor parameters, and fails with a `conflict` cause when that agent exists; otherwise the new agent has no key and a new ID. `config` overrides local configuration values, and `env` adds environment variables to the ones the manifest defines.",
        );
        let mut method = writer.begin_static_method("create");
        inner.write_parameter_list(&mut method, &inner.agent_type.constructor.input_schema)?;
        method.param(
            &format!("{options}?"),
            &inner.create_options_type(&configs, false)?,
        );
        method.result(&format!(
            "Effect.Effect<{class}, base.RemoteCallError, base.ConnectionRequirements>"
        ));
        method.write_line("return Effect.gen(function* () {");
        method.write_line(format!(
            "const [{payload}, {config}] = yield* base.attempt(() => {{"
        ));
        method.write_line(format!("const {payload}: base.SchemaValue = "));
        inner.write_encode_input_record(
            &mut method,
            &inner.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        inner.write_config_option_locals(&mut method, &options, &configs, &config_names);
        inner.write_guest_config_encoding(&mut method, &configs, &config_names, &config)?;
        method.write_line(format!("return [{payload}, {config}] as const; }});"));
        method.write_line(format!(
            "return new {class}(yield* base.createRemoteAgent({}, {payload}, {{ keyed: {options}?.keyed ?? false, configs: {config}, env: {options}?.env ?? {{}} }}));",
            serde_json::to_string(class)?
        ));
        method.write_line("});");
        Ok(())
    }
}

// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::*;

pub struct EffectGuestBridgeGenerator {
    inner: TypeScriptBridgeGenerator,
}

impl BridgeGenerator for EffectGuestBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: TypeScriptBridgeGenerator::new_guest_with_extra_reserved_names(
                agent_type,
                target_path,
                testing,
                vec!["Effect".into()],
            )?,
        })
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        let inner = &self.inner;
        std::fs::create_dir_all(&inner.target_path)?;
        let library = inner.library_name();
        let mut body = TsWriter::new();
        inner.generate_ts_type_definitions(&mut body)?;
        self.class(&mut body)?;
        let mut writer = TsWriter::new();
        writer.write_line("import { Effect } from 'effect';");
        writer.write_line("import { Bridge as base } from '@golemcloud/effect-golem';");
        writer.write_line(inner.schema_graphs.borrow().definitions());
        writer.write_line(body.finish_string());
        writer.finish(&inner.target_path.join(format!("{library}.ts")))?;
        let dependency = sdk_overrides()?.effect_golem_dep()?;
        let package = json!({
            "name": library, "version": "0.0.1", "type": "module",
            "main": format!("{library}.js"), "types": format!("{library}.d.ts"),
            "scripts": { "build": "tsc" },
            "dependencies": { "@golemcloud/effect-golem": dependency, "effect": "4.0.0-beta.98" },
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
            "private constructor(private readonly resolved: base.RemoteAgentHandle) {}",
        );
        let durable = inner.agent_type.mode == AgentMode::Durable;
        if durable {
            writer.write_line("get agentId(): string { return this.resolved.agentId; }");
        }
        let configs: Vec<_> = inner
            .agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .collect();
        for with_config in [false, true] {
            if with_config && configs.is_empty() {
                continue;
            }
            for (name, explicit, fresh) in [
                ("get", false, false),
                ("getPhantom", true, false),
                ("newPhantom", false, true),
            ] {
                if !durable && name != "newPhantom" {
                    continue;
                }
                let name = format!("{name}{}", if with_config { "WithConfig" } else { "" });
                self.constructor(
                    writer,
                    &name,
                    explicit,
                    fresh && durable,
                    if with_config { &configs } else { &[] },
                )?;
            }
        }
        let mut naming = ParameterNaming::new();
        naming.reserve_many(["resolved", "constructor", "agentId"]);
        for method in &inner.agent_type.methods {
            let member = naming.fresh(inner.to_js_ident(&method.name));
            let args = inner.input_type_list(&method.input_schema)?;
            let encode = inner.build_encode_args_fn(method)?;
            let decode = inner.build_guest_decode_result_fn(method)?;
            let name = serde_json::to_string(&method.name)?;
            let await_name = if durable {
                "invokeAndAwait"
            } else {
                "invokeAndAwaitWithMetadata"
            };
            let trigger = if durable {
                "invoke"
            } else {
                "invokeWithMetadata"
            };
            let schedule = if durable {
                "scheduleCancelable"
            } else {
                "scheduleCancelableWithMetadata"
            };
            writer.write_line(format!(
                r#"
readonly {member} = (() => {{
  const __encode = {encode};
  const __decode = {decode};
  const __call = (...args: [{args}]) => base.attempt(() => __encode(args)).pipe(
    Effect.flatMap(input => this.resolved.{await_name}({name}, input, __decode)));
"#
            ));
            if method.uses_streams(&inner.agent_type.schema) {
                writer.write_line("return __call; })();");
                continue;
            }
            writer.write_line(format!(r#"
  const schedule = (at: Parameters<base.RemoteAgentHandle['scheduleCancelable']>[0], ...args: [{args}]) =>
    base.attempt(() => __encode(args)).pipe(Effect.flatMap(input => this.resolved.{schedule}(at, {name}, input)));
  return Object.assign(__call, {{
    trigger: (...args: [{args}]) => base.attempt(() => __encode(args)).pipe(
      Effect.flatMap(input => this.resolved.{trigger}({name}, input))),
    schedule,
    scheduleCancelable: schedule,
  }});
}})();
"#));
        }
        writer.end_export_class();
        Ok(())
    }

    fn constructor(
        &self,
        writer: &mut TsWriter,
        name: &str,
        explicit: bool,
        fresh: bool,
        configs: &[&AgentConfigDeclarationSchema],
    ) -> anyhow::Result<()> {
        let inner = &self.inner;
        let class = &inner.agent_type.type_name.0;
        let mut naming = ParameterNaming::new();
        match inner.ts_input(&inner.agent_type.constructor.input_schema)? {
            TsInput::Params(params) => naming.reserve_many(params.into_iter().map(|(n, _)| n)),
            TsInput::Multimodal(_) => naming.reserve(MULTIMODAL_INPUT_NAME),
        }
        let config_names: Vec<_> = configs
            .iter()
            .map(|c| naming.fresh(TypeScriptBridgeGenerator::guest_config_parameter_name(c)))
            .collect();
        let phantom = naming.fresh("phantomId");
        let payload = naming.fresh("constructorPayload");
        let config = naming.fresh("agentConfig");
        let mut method = writer.begin_static_method(name);
        if explicit {
            method.param(&phantom, "string");
        }
        inner.write_parameter_list(&mut method, &inner.agent_type.constructor.input_schema)?;
        inner.write_guest_config_parameter_list(&mut method, configs, &config_names)?;
        method.result(&format!(
            "Effect.Effect<{class}, base.RemoteCallError, base.ConnectionRequirements{}>",
            if fresh {
                " | base.PhantomRequirements"
            } else {
                ""
            }
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
        inner.write_guest_config_encoding(&mut method, configs, &config_names, &config)?;
        method.write_line(format!("return [{payload}, {config}] as const; }});"));
        if fresh {
            method.write_line(format!("const {phantom} = yield* base.generatePhantomId;"));
        } else if !explicit {
            method.write_line(format!("const {phantom} = undefined;"));
        }
        let mode = if inner.agent_type.mode == AgentMode::Durable {
            "durable"
        } else {
            "ephemeral"
        };
        method.write_line(format!("return new {class}(yield* base.resolveRemoteAgent({}, {payload}, {phantom}, {config}, {mode:?}));", serde_json::to_string(class)?));
        method.write_line("});");
        Ok(())
    }
}

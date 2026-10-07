test_r::enable!();

#[cfg(test)]
#[allow(dead_code, private_interfaces)]
mod fixture {
    use golem_rust::agentic::{InputStream, OutputStream, ToolBuildCtx};
    use golem_rust::{
        FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use test_r::test;

    macro_rules! anonymous_enum {
        ($name:ident { $($variant:ident = ($index:literal, $case:literal)),+ $(,)? }) => {
            #[derive(Clone)]
            enum $name { $($variant),+ }

            impl IntoSchema for $name {
                fn type_id() -> golem_rust::schema::TypeId {
                    golem_rust::schema::TypeId::new(stringify!($name))
                }

                fn register_in(_: &mut golem_rust::schema::SchemaBuilder) -> golem_rust::SchemaType {
                    golem_rust::SchemaType::r#enum(vec![$($case.to_string()),+])
                }

                fn to_value(&self) -> golem_rust::SchemaValue {
                    golem_rust::SchemaValue::Enum { case: match self { $(Self::$variant => $index),+ } }
                }
            }

            impl FromSchema for $name {
                fn from_value(value: &golem_rust::SchemaValue) -> Result<Self, golem_rust::schema::FromSchemaError> {
                    match value {
                        golem_rust::SchemaValue::Enum { case } => match case { $($index => Ok(Self::$variant),)+ _ => Err(golem_rust::schema::FromSchemaError::custom("enum case out of range")) },
                        _ => Err(golem_rust::schema::FromSchemaError::custom("expected enum")),
                    }
                }
            }

            impl WireSchema for $name {
                fn append_schema(builder: &mut golem_rust::schema::wit::direct::WireSchemaBuilder) -> i32 {
                    builder.push(golem_rust::schema::wit::wire::SchemaTypeBody::EnumType(vec![$($case.to_string()),+]))
                }
            }

            impl IntoWire for $name {
                fn write_wire(&self, writer: &mut golem_rust::schema::wit::direct::WireWriter) -> Result<i32, golem_rust::schema::wit::direct::WireError> {
                    Ok(writer.push(golem_rust::schema::wit::wire::SchemaValueNode::EnumValue(match self { $(Self::$variant => $index),+ })))
                }
            }

            impl FromWire for $name {
                fn read_wire(reader: &mut golem_rust::schema::wit::direct::WireReader, index: i32) -> Result<Self, golem_rust::schema::wit::direct::WireError> {
                    match reader.take(index)? {
                        golem_rust::schema::wit::wire::SchemaValueNode::EnumValue(case) => match case { $($index => Ok(Self::$variant),)+ _ => Err(golem_rust::schema::wit::direct::WireError::Shape("enum case")) },
                        _ => Err(golem_rust::schema::wit::direct::WireError::Shape("enum")),
                    }
                }
            }
        };
    }

    anonymous_enum!(Region { EuWest1 = (0, "eu-west-1"), UsEast1 = (1, "us-east-1") });
    anonymous_enum!(Profile { Debug = (0, "debug"), Release = (1, "release") });
    anonymous_enum!(ReportFormat { Json = (0, "json"), Text = (1, "text") });
    anonymous_enum!(ColorMode { Auto = (0, "auto"), Always = (1, "always"), Never = (2, "never") });
    anonymous_enum!(RenderStatus { Queued = (0, "queued"), Ready = (1, "ready"), Failed = (2, "failed") });

    #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    struct ArtifactRequest {
        source: String,
        labels: BTreeMap<String, String>,
    }

    #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    #[schema(rename_all = "camelCase")]
    struct ArtifactReport {
        artifact_id: u64,
        #[schema(text(min = 8, max = 64, regex = "^[a-f0-9]+$"))]
        digest: String,
        labels: BTreeMap<String, String>,
        warnings: Vec<String>,
    }

    #[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
    struct ValidationFailure {
        field: String,
        reason: String,
        retryable: bool,
    }

    struct RenderFailure {
        stage: String,
        code: u32,
    }

    impl IntoSchema for RenderFailure {
        fn type_id() -> golem_rust::schema::TypeId {
            golem_rust::schema::TypeId::new("RenderFailure")
        }

        fn register_in(builder: &mut golem_rust::schema::SchemaBuilder) -> golem_rust::SchemaType {
            golem_rust::SchemaType::record_from_fields([
                ("stage", String::register_in(builder)),
                ("code", u32::register_in(builder)),
            ])
        }

        fn to_value(&self) -> golem_rust::SchemaValue {
            golem_rust::SchemaValue::Record {
                fields: vec![self.stage.to_value(), self.code.to_value()],
            }
        }
    }

    impl FromSchema for RenderFailure {
        fn from_value(
            value: &golem_rust::SchemaValue,
        ) -> Result<Self, golem_rust::schema::FromSchemaError> {
            let golem_rust::SchemaValue::Record { fields } = value else {
                return Err(golem_rust::schema::FromSchemaError::custom(
                    "expected record",
                ));
            };
            if fields.len() != 2 {
                return Err(golem_rust::schema::FromSchemaError::custom(
                    "expected two fields",
                ));
            }
            Ok(Self {
                stage: String::from_value(&fields[0])?,
                code: u32::from_value(&fields[1])?,
            })
        }
    }

    impl WireSchema for RenderFailure {
        fn append_schema(builder: &mut golem_rust::schema::wit::direct::WireSchemaBuilder) -> i32 {
            let stage = String::append_schema(builder);
            let code = u32::append_schema(builder);
            builder.push(golem_rust::schema::wit::wire::SchemaTypeBody::RecordType(
                vec![
                    golem_rust::schema::wit::wire::NamedFieldType {
                        name: "stage".to_string(),
                        body: stage,
                        metadata: golem_rust::schema::wit::direct::empty_metadata(),
                    },
                    golem_rust::schema::wit::wire::NamedFieldType {
                        name: "code".to_string(),
                        body: code,
                        metadata: golem_rust::schema::wit::direct::empty_metadata(),
                    },
                ],
            ))
        }
    }

    impl IntoWire for RenderFailure {
        fn write_wire(
            &self,
            writer: &mut golem_rust::schema::wit::direct::WireWriter,
        ) -> Result<i32, golem_rust::schema::wit::direct::WireError> {
            let stage = self.stage.write_wire(writer)?;
            let code = self.code.write_wire(writer)?;
            Ok(
                writer.push(golem_rust::schema::wit::wire::SchemaValueNode::RecordValue(
                    vec![stage, code],
                )),
            )
        }
    }

    impl FromWire for RenderFailure {
        fn read_wire(
            reader: &mut golem_rust::schema::wit::direct::WireReader,
            index: i32,
        ) -> Result<Self, golem_rust::schema::wit::direct::WireError> {
            let golem_rust::schema::wit::wire::SchemaValueNode::RecordValue(fields) =
                reader.take(index)?
            else {
                return Err(golem_rust::schema::wit::direct::WireError::Shape("record"));
            };
            if fields.len() != 2 {
                return Err(golem_rust::schema::wit::direct::WireError::Shape(
                    "field count",
                ));
            }
            Ok(Self {
                stage: String::read_wire(reader, fields[0])?,
                code: u32::read_wire(reader, fields[1])?,
            })
        }
    }

    #[derive(ToolError)]
    enum RenderError {
        /// Request validation failed
        #[tool_error(kind = "usage-error", exit_code = 2)]
        InvalidRequest(ValidationFailure),
        /// Renderer failed
        #[tool_error(kind = "runtime-error", exit_code = 70)]
        RenderFailed(RenderFailure),
    }

    struct RenderSubtree;

    /// Build and inspect artifacts
    ///
    /// A deliberately asymmetric conformance tool.
    #[tool_definition(version = "1.0.0", aliases = ["art"])]
    #[example(
        title = "Render",
        body = "artifact --region eu-west-1 render src/main.wasm --format json"
    )]
    trait Artifact {
        /// Render one artifact
        ///
        /// Build an artifact and return a structured report.
        #[example(
            title = "Release build",
            body = "artifact render src/main.wasm --format json --tag release --define opt=3 --checksum"
        )]
        #[command(subtree = Render, aliases = ["build"])]
        #[arg(
            region = "root-global",
            short = 'r',
            aliases = ["location"],
            value_name = "REGION",
            default = "eu-west-1",
            env = "ARTIFACT_REGION",
            doc = "Execution region",
            description = "Inherited by every executable descendant."
        )]
        #[arg(
            trace = "root-global",
            short = 't',
            kind = "flag",
            aliases = ["diagnostics"],
            negatable = true,
            default = false,
            doc = "Emit trace details"
        )]
        #[arg(
            profile = "global",
            short = 'p',
            value_name = "PROFILE",
            default = "release",
            doc = "Build profile",
            description = "Inherited by render descendants."
        )]
        fn render(&self, region: Region, trace: bool, profile: Profile) -> RenderSubtree;
    }

    #[tool_definition]
    trait Render {
        #[arg(region = "global", aliases = ["location"], default = "eu-west-1")]
        #[arg(trace = "global", kind = "flag", aliases = ["diagnostics"], negatable = true, default = false)]
        #[arg(profile = "global", default = "release")]
        #[arg(
            request = "positional",
            value_name = "REQUEST",
            doc = "Artifact request"
        )]
        #[arg(
            inputs = "tail",
            value_name = "INPUT",
            kind = "file",
            direction = "input",
            extensions = ["wasm", "wat"],
            min = 1,
            max = 3,
            separator = "--",
            verbatim,
            doc = "Input modules",
            description = "One to three source modules."
        )]
        #[arg(
            format = "option",
            short = 'f',
            aliases = ["output-format"],
            value_name = "FORMAT",
            default = "json",
            doc = "Report format"
        )]
        #[arg(
            tag = "option",
            aliases = ["label"],
            value_name = "TAG",
            repeatable = "either",
            delim = ',',
            default = [],
            doc = "Tags",
            description = "May be repeated or comma-delimited."
        )]
        #[arg(
            define = "option",
            short = 'D',
            value_name = "KEY=VALUE",
            repeatable = "repeated",
            default = [],
            doc = "Numeric definitions",
            description = "Duplicate keys are rejected."
        )]
        #[arg(
            color = "option",
            value_name = "WHEN",
            optional_scalar,
            default = "auto",
            doc = "Color mode",
            description = "Bare presence resolves to the default."
        )]
        #[arg(
            checksum = "flag",
            short = 'c',
            aliases = ["digest"],
            negatable = true,
            default = false,
            doc = "Include digest"
        )]
        #[arg(
            verbose = "flag",
            short = 'v',
            kind = "count-flag",
            max = 3,
            doc = "Verbosity"
        )]
        #[arg(stdin, mime = ["application/wasm"], doc = "Optional module bytes")]
        #[arg(stdout, mime = ["text/plain; charset=utf-8"], doc = "Progress output")]
        #[arg(
            stderr,
            channel = "stderr",
            mime = ["application/octet-stream"],
            doc = "Diagnostic bytes"
        )]
        #[constraint(requires_all = ["checksum", "format"])]
        #[constraint(implies(
            lhs = value_is("profile", "release"),
            rhs = "tag",
            rhs_quant = "any"
        ))]
        #[constraint(forbids(lhs = value_is("format", "text"), rhs = "define", lhs_quant = "any"))]
        #[command(annotations(
            read_only = false,
            destructive = false,
            idempotent = true,
            open_world = false
        ))]
        #[result(
            formatters = [("json", "JSON report"), ("table", "Tabular report")],
            default = "json",
            doc = "Artifact report"
        )]
        #[allow(clippy::too_many_arguments)]
        fn render(
            &self,
            region: Region,
            trace: bool,
            profile: Profile,
            request: ArtifactRequest,
            inputs: Vec<PathBuf>,
            format: ReportFormat,
            tag: Vec<String>,
            define: BTreeMap<String, i64>,
            color: ColorMode,
            checksum: bool,
            verbose: u32,
            stdin: Option<InputStream>,
            stdout: OutputStream,
            stderr: Option<OutputStream>,
        ) -> Result<ArtifactReport, RenderError>;

        /// Inspect render status
        #[command(aliases = ["show"], annotations(
            read_only = true,
            destructive = false,
            idempotent = true,
            open_world = false
        ))]
        #[arg(region = "global", aliases = ["location"], default = "eu-west-1")]
        #[arg(trace = "global", kind = "flag", aliases = ["diagnostics"], negatable = true, default = false)]
        #[arg(profile = "global", default = "release")]
        #[arg(
            artifact_id = "positional",
            value_name = "ID",
            doc = "Artifact identifier"
        )]
        #[result(
            formatters = [("json", "JSON status")],
            default = "json",
            doc = "Current status"
        )]
        fn status(
            &self,
            region: Region,
            trace: bool,
            profile: Profile,
            artifact_id: u64,
        ) -> RenderStatus;
    }

    fn type_json(ty: &golem_rust::SchemaType, names: &BTreeMap<String, String>) -> Value {
        use golem_rust::SchemaType;

        match ty {
            SchemaType::Ref { id, .. } => json!({ "kind": "ref", "id": names[&id.to_string()] }),
            SchemaType::Bool { .. } => json!({ "kind": "bool" }),
            SchemaType::S64 { .. } => json!({ "kind": "s64" }),
            SchemaType::U32 { .. } => json!({ "kind": "u32" }),
            SchemaType::U64 { .. } => json!({ "kind": "u64" }),
            SchemaType::String { .. } => json!({ "kind": "string" }),
            SchemaType::Record { fields, .. } => json!({
                "kind": "record",
                "fields": fields.iter().map(|field| json!({
                    "name": field.name,
                    "type": type_json(&field.body, names),
                })).collect::<Vec<_>>(),
            }),
            SchemaType::Enum { cases, .. } => json!({ "kind": "enum", "cases": cases }),
            SchemaType::List { element, .. } => {
                json!({ "kind": "list", "item": type_json(element, names) })
            }
            SchemaType::Map { key, value, .. } => json!({
                "kind": "map",
                "key": type_json(key, names),
                "value": type_json(value, names),
            }),
            SchemaType::Text { restrictions, .. } => json!({
                "kind": "text",
                "restrictions": {
                    "minLength": restrictions.min_length,
                    "maxLength": restrictions.max_length,
                    "regex": restrictions.regex,
                },
            }),
            SchemaType::Path { spec, .. } => json!({
                "kind": "path",
                "direction": serde_json::to_value(spec.direction).unwrap(),
                "pathKind": serde_json::to_value(spec.kind).unwrap(),
                "extensions": spec.allowed_extensions,
            }),
            other => panic!("schema kind not used by rich fixture: {other:?}"),
        }
    }

    fn value_json(
        value: &golem_rust::SchemaValue,
        ty: &golem_rust::SchemaType,
        graph: &golem_rust::SchemaGraph,
    ) -> Value {
        use golem_rust::{SchemaType, SchemaValue};

        let ty = match ty {
            SchemaType::Ref { id, .. } => &graph.lookup(id).unwrap().body,
            ty => ty,
        };
        match (value, ty) {
            (SchemaValue::Bool(value), _) => json!(value),
            (SchemaValue::String(value), _) => json!(value),
            (SchemaValue::S64(value), _) => json!(value),
            (SchemaValue::Enum { case }, SchemaType::Enum { cases, .. }) => {
                json!(cases[*case as usize])
            }
            (SchemaValue::List { elements }, SchemaType::List { element, .. }) => Value::Array(
                elements
                    .iter()
                    .map(|value| value_json(value, element, graph))
                    .collect(),
            ),
            (SchemaValue::Map { entries }, SchemaType::Map { key, value, .. }) => Value::Object(
                entries
                    .iter()
                    .map(|(entry_key, entry_value)| {
                        let Value::String(entry_key) = value_json(entry_key, key, graph) else {
                            panic!("rich fixture map key is not a string")
                        };
                        (entry_key, value_json(entry_value, value, graph))
                    })
                    .collect(),
            ),
            _ => panic!("value does not match rich fixture schema: {value:?} / {ty:?}"),
        }
    }

    fn repetition_json(repetition: &golem_rust::schema::tool::Repetition) -> Value {
        use golem_rust::schema::tool::Repetition;
        match repetition {
            Repetition::Repeated => json!({ "kind": "repeated" }),
            Repetition::Delimited(delimiter) => {
                json!({ "kind": "delimited", "delimiter": delimiter.to_string() })
            }
            Repetition::Either(delimiter) => {
                json!({ "kind": "either", "delimiter": delimiter.to_string() })
            }
        }
    }

    fn option_shape_json(
        shape: &golem_rust::schema::tool::OptionShape,
        names: &BTreeMap<String, String>,
    ) -> Value {
        use golem_rust::schema::tool::OptionShape;
        match shape {
            OptionShape::Scalar(ty) => json!({ "kind": "scalar", "type": type_json(ty, names) }),
            OptionShape::OptionalScalar(ty) => {
                json!({ "kind": "optional-scalar", "type": type_json(ty, names) })
            }
            OptionShape::RepeatableList(shape) => json!({
                "kind": "repeatable-list",
                "repetition": repetition_json(&shape.repetition),
                "itemType": type_json(&shape.item_type, names),
            }),
            OptionShape::RepeatableMap(shape) => json!({
                "kind": "repeatable-map",
                "repetition": repetition_json(&shape.repetition),
                "mapType": type_json(&shape.map_type, names),
                "duplicateKeyPolicy": serde_json::to_value(shape.duplicate_key_policy).unwrap(),
            }),
        }
    }

    fn option_json(
        option: &golem_rust::schema::tool::OptionSpec,
        graph: &golem_rust::SchemaGraph,
        names: &BTreeMap<String, String>,
    ) -> Value {
        use golem_rust::schema::tool::OptionShape;
        let value_type = match &option.shape {
            OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
            OptionShape::RepeatableList(shape) => {
                return json!({
                    "long": option.long, "short": option.short.map(|x| x.to_string()), "aliases": option.aliases,
                    "valueName": option.value_name, "shape": option_shape_json(&option.shape, names),
                    "default": option.default.as_ref().map(|value| value_json(value, &golem_rust::SchemaType::List { element: Box::new(shape.item_type.clone()), metadata: Default::default() }, graph)),
                    "required": option.required, "envVar": option.env_var, "doc": option.doc,
                });
            }
            OptionShape::RepeatableMap(shape) => &shape.map_type,
        };
        json!({
            "long": option.long, "short": option.short.map(|x| x.to_string()), "aliases": option.aliases,
            "valueName": option.value_name, "shape": option_shape_json(&option.shape, names),
            "default": option.default.as_ref().map(|value| value_json(value, value_type, graph)),
            "required": option.required, "envVar": option.env_var, "doc": option.doc,
        })
    }

    fn flag_json(flag: &golem_rust::schema::tool::FlagSpec) -> Value {
        use golem_rust::schema::tool::FlagShape;
        let shape = match flag.shape {
            FlagShape::BoolFlag(shape) => {
                json!({ "kind": "bool-flag", "default": shape.default, "negatable": shape.negatable })
            }
            FlagShape::CountFlag(max) => json!({ "kind": "count-flag", "max": max }),
        };
        json!({
            "long": flag.long, "short": flag.short.map(|x| x.to_string()), "aliases": flag.aliases,
            "shape": shape, "envVar": flag.env_var, "doc": flag.doc,
        })
    }

    fn ref_json(
        reference: &golem_rust::schema::tool::Ref,
        graph: &golem_rust::SchemaGraph,
    ) -> Value {
        use golem_rust::schema::tool::Ref;
        match reference {
            Ref::Present(name) => json!({ "kind": "present", "name": name }),
            Ref::ValueIs(value) => {
                let ty = match value.name.as_str() {
                    "profile" => {
                        golem_rust::SchemaType::r#enum(vec!["debug".into(), "release".into()])
                    }
                    "format" => golem_rust::SchemaType::r#enum(vec!["json".into(), "text".into()]),
                    name => panic!("value-is type not used by rich fixture: {name}"),
                };
                json!({
                    "kind": "value-is", "name": value.name,
                    "value": value_json(&value.value, &ty, graph),
                })
            }
        }
    }

    fn constraint_json(
        constraint: &golem_rust::schema::tool::Constraint,
        graph: &golem_rust::SchemaGraph,
    ) -> Value {
        use golem_rust::schema::tool::Constraint;
        match constraint {
            Constraint::RequiresAll(refs) => {
                json!({ "kind": "requires-all", "refs": refs.iter().map(|x| ref_json(x, graph)).collect::<Vec<_>>() })
            }
            Constraint::Implies(value) => json!({
                "kind": "implies", "lhsQuantifier": serde_json::to_value(value.lhs_quant).unwrap(),
                "lhs": value.lhs.iter().map(|x| ref_json(x, graph)).collect::<Vec<_>>(),
                "rhsQuantifier": serde_json::to_value(value.rhs_quant).unwrap(),
                "rhs": value.rhs.iter().map(|x| ref_json(x, graph)).collect::<Vec<_>>(),
            }),
            Constraint::Forbids(value) => json!({
                "kind": "forbids", "lhsQuantifier": serde_json::to_value(value.lhs_quant).unwrap(),
                "lhs": value.lhs.iter().map(|x| ref_json(x, graph)).collect::<Vec<_>>(),
                "rhs": value.rhs.iter().map(|x| ref_json(x, graph)).collect::<Vec<_>>(),
            }),
            other => panic!("constraint not used by rich fixture: {other:?}"),
        }
    }

    fn command_json(
        index: usize,
        tool: &golem_rust::schema::tool::Tool,
        names: &BTreeMap<String, String>,
    ) -> Value {
        let node = &tool.commands.nodes[index];
        let globals = json!({
            "options": node.globals.options.iter().map(|x| option_json(x, &tool.schema, names)).collect::<Vec<_>>(),
            "flags": node.globals.flags.iter().map(flag_json).collect::<Vec<_>>(),
        });
        let body = node.body.as_ref().map(|body| json!({
            "positionals": {
                "fixed": body.positionals.fixed.iter().map(|x| json!({
                    "name": x.name, "valueName": x.value_name, "type": type_json(&x.type_, names),
                    "default": x.default.as_ref().map(|value| value_json(value, &x.type_, &tool.schema)),
                    "required": x.required, "acceptsStdio": x.accepts_stdio, "doc": x.doc,
                })).collect::<Vec<_>>(),
                "tail": body.positionals.tail.as_ref().map(|x| json!({
                    "name": x.name, "valueName": x.value_name, "itemType": type_json(&x.item_type, names),
                    "min": x.min, "max": x.max, "separator": x.separator, "verbatim": x.verbatim,
                    "acceptsStdio": x.accepts_stdio, "doc": x.doc,
                })),
            },
            "options": body.options.iter().map(|x| option_json(x, &tool.schema, names)).collect::<Vec<_>>(),
            "flags": body.flags.iter().map(flag_json).collect::<Vec<_>>(),
            "constraints": body.constraints.iter().map(|x| constraint_json(x, &tool.schema)).collect::<Vec<_>>(),
            "stdin": body.stdin, "stdout": body.stdout, "stderr": body.stderr,
            "result": body.result.as_ref().map(|x| json!({
                "type": type_json(&x.type_, names), "formatters": x.formatters, "defaultFormatter": x.default_formatter, "doc": x.doc,
            })),
            "errors": body.errors.iter().map(|x| json!({
                "name": x.name, "kind": serde_json::to_value(x.kind).unwrap(), "exitCode": x.exit_code,
                "payload": x.payload.as_ref().map(|payload| type_json(payload, names)), "doc": x.doc,
            })).collect::<Vec<_>>(),
            "annotations": body.annotations.as_ref().map(|x| json!({
                "readOnly": x.read_only, "destructive": x.destructive, "idempotent": x.idempotent, "openWorld": x.open_world,
            })),
        }));
        json!({
            "name": node.name, "aliases": node.aliases, "doc": node.doc, "globals": globals, "body": body,
            "subcommands": node.subcommands.iter().map(|child| command_json(child.as_usize().unwrap(), tool, names)).collect::<Vec<_>>(),
        })
    }

    fn oracle_json(tool: &golem_rust::schema::tool::Tool) -> Value {
        let names = tool
            .schema
            .defs
            .iter()
            .map(|def| (def.id.to_string(), def.name.clone().unwrap()))
            .collect::<BTreeMap<_, _>>();
        let definitions = tool
            .schema
            .defs
            .iter()
            .map(|def| (def.name.clone().unwrap(), type_json(&def.body, &names)))
            .collect::<serde_json::Map<_, _>>();
        json!({
            "version": tool.version, "requiresFilesystem": tool.requires_filesystem,
            "name": tool.name().unwrap(), "schemaDefinitions": definitions,
            "commands": command_json(0, tool, &names),
        })
    }

    #[test]
    fn emitted_metadata_matches_rich_tool_oracle() {
        let tool = __golem_tool_descriptor_for_Artifact(&mut ToolBuildCtx::new())
            .unwrap()
            .try_to_native_tool()
            .unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../../test-data/gol-40/rich-tool-conformance-v1.json"
        ))
        .unwrap();
        assert_eq!(oracle_json(&tool), expected["tool"]);
    }

    #[tool_definition]
    trait SharedRootGlobal {
        #[arg(region = "root-global", default = "eu-west-1")]
        fn build(&self, region: String, input: String) -> String;

        #[arg(region = "root-global", default = "eu-west-1")]
        fn inspect(&self, region: String, input: String) -> String;
    }

    #[test]
    fn repeated_root_global_declarations_are_merged_at_dispatcher_root() {
        let tool = __golem_tool_descriptor_for_SharedRootGlobal(&mut ToolBuildCtx::new())
            .expect("matching root-global declarations should build")
            .try_to_native_tool()
            .expect("matching root-global declarations should produce valid native metadata");

        assert_eq!(tool.commands.nodes[0].globals.options.len(), 1);
        assert_eq!(tool.commands.nodes[0].globals.options[0].long, "region");
    }

    #[tool_definition]
    trait AliasSharedRootGlobal {
        #[arg(region = "root-global", aliases = ["location"], default = "eu-west-1")]
        fn build(&self, region: String, input: String) -> String;

        #[arg(location = "root-global", default = "eu-west-1")]
        fn inspect(&self, location: String, input: String) -> String;
    }

    #[test]
    fn alias_matching_root_global_declarations_are_merged() {
        let tool = __golem_tool_descriptor_for_AliasSharedRootGlobal(&mut ToolBuildCtx::new())
            .expect("alias-matching root-global declarations should build")
            .try_to_native_tool()
            .expect("alias-matching root-global declarations should produce valid metadata");

        assert_eq!(tool.commands.nodes[0].globals.options.len(), 1);
    }
}

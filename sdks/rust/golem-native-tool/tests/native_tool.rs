use golem_native_tool::{
    HostResult, NativeToolInvocation, NativeToolInvoker, NativeToolRpcError, NativeToolStdin,
    NativeToolStdinHandle, NativeToolStdout, NativeToolStdoutHandle, Principal, SchemaValue,
    ToolError, TypedSchemaValue, tool_definition, tool_implementation,
};
use std::future::Future;
use std::pin::Pin;
use test_r::test;

test_r::enable!();

#[tool_definition(version = "1.2.3")]
trait Counter {
    /// Adds a value to the host-owned counter.
    async fn counter(&self, context: &mut u64, value: u64) -> u64;
}

struct CounterImpl;

#[tool_implementation]
impl Counter for CounterImpl {
    async fn counter(&self, context: &mut u64, value: u64) -> u64 {
        *context += value;
        *context
    }
}

#[test]
async fn metadata_and_send_invocation_share_sdk_authoring() {
    let invoker = __GolemNativeToolInvokerCounterImplCounter::new(CounterImpl);
    let metadata = invoker.metadata();
    let definition = invoker.definition("counter", "7").unwrap();
    assert_eq!(definition.tool, metadata);
    assert_eq!(definition.id, "counter");
    assert_eq!(definition.implementation_version, "7");
    definition.validate().unwrap();
    let mut mismatched = definition.clone();
    mismatched.metadata_digest[0] ^= 1;
    assert!(mismatched.validate().is_err());
    assert_eq!(metadata.version, "1.2.3");
    assert_eq!(metadata.commands.nodes[0].name, "counter");
    assert_eq!(
        metadata.commands.nodes[0].doc.summary,
        "Adds a value to the host-owned counter."
    );

    let model = metadata.canonical_input_model(0).unwrap();
    let input = TypedSchemaValue::new(
        model.record_schema,
        SchemaValue::Record {
            fields: vec![SchemaValue::U64(4)],
        },
    );
    let mut state = 3u64;
    let result = invoker
        .invoke(
            &mut state,
            NativeToolInvocation {
                command_path: vec![],
                input,
                principal: Principal::Anonymous,
                stdin: None,
                stdout: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.result.unwrap().value(), &SchemaValue::U64(7));
}

#[derive(Debug, ToolError)]
enum DeclaredError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    Rejected,
}

#[tool_definition]
trait FallibleNative {
    fn unit(&self, context: &mut FallibleContext, fail: bool) -> HostResult<()>;
    fn separated(
        &self,
        context: &mut FallibleContext,
        infrastructure_failure: bool,
        declared_failure: bool,
    ) -> HostResult<Result<u64, DeclaredError>>;
}

struct FallibleNativeImpl;

#[derive(Default)]
struct FallibleContext {
    called: bool,
}

impl FallibleContext {
    fn host_function(&mut self, fail: bool) -> HostResult<()> {
        self.called = true;
        if fail {
            anyhow::bail!("host function failed")
        }
        Ok(())
    }
}

#[tool_implementation]
impl FallibleNative for FallibleNativeImpl {
    fn unit(&self, context: &mut FallibleContext, fail: bool) -> HostResult<()> {
        context.host_function(fail)?;
        Ok(())
    }

    fn separated(
        &self,
        context: &mut FallibleContext,
        infrastructure_failure: bool,
        declared_failure: bool,
    ) -> HostResult<Result<u64, DeclaredError>> {
        context.called = true;
        if infrastructure_failure {
            anyhow::bail!("infrastructure failure")
        } else if declared_failure {
            Ok(Err(DeclaredError::Rejected))
        } else {
            Ok(Ok(42))
        }
    }
}

fn invocation(
    metadata: &golem_native_tool::Tool,
    command: &str,
    fields: Vec<SchemaValue>,
) -> NativeToolInvocation {
    let index = metadata
        .command_index_by_path(&[command.to_string()])
        .unwrap();
    let model = metadata.canonical_input_model(index).unwrap();
    NativeToolInvocation {
        command_path: vec![command.to_string()],
        input: TypedSchemaValue::new(model.record_schema, SchemaValue::Record { fields }),
        principal: Principal::Anonymous,
        stdin: None,
        stdout: None,
    }
}

#[test]
async fn host_result_unit_has_a_distinct_infrastructure_channel() {
    let invoker = __GolemNativeToolInvokerFallibleNativeImplFallibleNative::new(FallibleNativeImpl);
    let metadata = invoker.metadata();
    let mut context = FallibleContext::default();
    assert!(
        invoker
            .invoke(
                &mut context,
                invocation(&metadata, "unit", vec![SchemaValue::Bool(false)])
            )
            .await
            .unwrap()
            .unwrap()
            .result
            .is_none()
    );
    assert!(context.called);

    let error = invoker
        .invoke(
            &mut context,
            invocation(&metadata, "unit", vec![SchemaValue::Bool(true)]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "host function failed");
}

#[test]
async fn host_result_keeps_declared_tool_errors_separate() {
    let invoker = __GolemNativeToolInvokerFallibleNativeImplFallibleNative::new(FallibleNativeImpl);
    let metadata = invoker.metadata();
    let mut context = FallibleContext::default();
    let declared = invoker
        .invoke(
            &mut context,
            invocation(
                &metadata,
                "separated",
                vec![SchemaValue::Bool(false), SchemaValue::Bool(true)],
            ),
        )
        .await
        .unwrap();
    assert!(matches!(declared, Err(NativeToolRpcError::Custom(_))));

    let infrastructure = invoker
        .invoke(
            &mut context,
            invocation(
                &metadata,
                "separated",
                vec![SchemaValue::Bool(true), SchemaValue::Bool(false)],
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(infrastructure.to_string(), "infrastructure failure");

    let success = invoker
        .invoke(
            &mut context,
            invocation(
                &metadata,
                "separated",
                vec![SchemaValue::Bool(false), SchemaValue::Bool(false)],
            ),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(success.result.unwrap().value(), &SchemaValue::U64(42));
}

#[tool_definition]
trait NativeChild {
    async fn run(
        &self,
        context: &mut Vec<String>,
        verbose: bool,
        name: String,
        principal: golem_native_tool::Principal,
        stdin: NativeToolStdin,
        stdout: Option<NativeToolStdout>,
    ) -> String;
}

struct NativeChildImpl;

#[tool_implementation]
impl NativeChild for NativeChildImpl {
    async fn run(
        &self,
        context: &mut Vec<String>,
        verbose: bool,
        name: String,
        principal: Principal,
        mut stdin: NativeToolStdin,
        mut stdout: Option<NativeToolStdout>,
    ) -> String {
        let bytes = stdin.read().await.unwrap().unwrap();
        if let Some(stdout) = &mut stdout {
            stdout.write(bytes).await.unwrap();
            stdout.finish().unwrap();
        }
        context.push(name.clone());
        format!("{verbose}:{name}:{principal:?}")
    }
}

#[tool_definition]
trait NativeParent {
    #[arg(verbose = "global", aliases = ["chatty"], kind = "flag")]
    #[command(name = "child", aliases = ["c"], subtree = NativeChild)]
    fn child(&self, context: &mut Vec<String>, verbose: bool) -> NativeChildImpl;
}

struct NativeParentImpl;

#[tool_implementation]
impl NativeParent for NativeParentImpl {
    fn child(&self, _context: &mut Vec<String>, _verbose: bool) -> NativeChildImpl {
        NativeChildImpl
    }
}

struct Input(Option<Vec<u8>>);
impl NativeToolStdinHandle for Input {
    fn read<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, String>>> + Send + 'a>> {
        Box::pin(async move { self.0.take().map(Ok) })
    }
}

struct Output;
impl NativeToolStdoutHandle for Output {
    fn write<'a>(
        &'a mut self,
        _bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
async fn subtree_routes_to_explicit_child_instance_with_inherited_globals_aliases_and_streams() {
    let invoker = __GolemNativeToolInvokerNativeParentImplNativeParent::new(NativeParentImpl);
    let metadata = invoker.metadata();
    let child = metadata
        .command_index_by_path(&["child".into(), "run".into()])
        .unwrap();
    let body = metadata.commands.nodes[child].body.as_ref().unwrap();
    assert_eq!(
        body.stdin.as_ref().map(|stream| stream.required),
        Some(true)
    );
    assert_eq!(
        body.stdout.as_ref().map(|stream| stream.required),
        Some(false)
    );
    let model = metadata.canonical_input_model(child).unwrap();
    let fields = model
        .fields
        .iter()
        .map(|field| match field.name.as_str() {
            "verbose" => SchemaValue::Bool(true),
            "name" => SchemaValue::String("Ada".into()),
            name => panic!("unexpected canonical field {name}"),
        })
        .collect();
    let input = TypedSchemaValue::new(model.record_schema, SchemaValue::Record { fields });
    let mut context = vec![];
    let result = invoker
        .invoke(
            &mut context,
            NativeToolInvocation {
                command_path: vec!["c".into(), "run".into()],
                input,
                principal: Principal::Anonymous,
                stdin: Some(Box::new(Input(Some(b"hello".to_vec())))),
                stdout: Some(Box::new(Output)),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context, ["Ada"]);
    assert_eq!(
        result.result.unwrap().value(),
        &SchemaValue::String("true:Ada:Anonymous".into())
    );
}

#[test]
fn both_generated_implementation_wrappers_expose_metadata() {
    assert_eq!(
        __GolemNativeToolInvokerNativeChildImplNativeChild::new(NativeChildImpl)
            .metadata()
            .commands
            .nodes[0]
            .name,
        "native-child"
    );
    assert_eq!(
        __GolemNativeToolInvokerNativeParentImplNativeParent::new(NativeParentImpl)
            .metadata()
            .commands
            .nodes[0]
            .name,
        "native-parent"
    );
}

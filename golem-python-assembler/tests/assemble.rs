//! Builds Python applications from the prepared SDK assets and runs them on
//! the shared engine.
//!
//! The prepared assets are a build output; `cargo make python-sdk-prepare`
//! writes them to `sdks/python/target/prepared`.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use golem_python_assembler::{Application, build, read_prepared};
use test_r::test;
use wasmtime::{
    Engine, Store,
    component::{Component, ComponentExportIndex, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{IoCtx, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

test_r::enable!();

struct Host {
    ctx: WasiCtx,
    io: IoCtx,
    table: ResourceTable,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
            io_ctx: &mut self.io,
        }
    }
}

fn python_sdk() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sdks/python")
}

fn prepared() -> Result<componentize_py::prepared::Prepared> {
    let dir = python_sdk().join("target/prepared");
    read_prepared(&dir).with_context(|| {
        format!(
            "run `cargo make python-sdk-prepare` to write the prepared assets to {}",
            dir.display()
        )
    })
}

fn write_app(dir: &std::path::Path, module: &str, source: &str) -> Result<()> {
    fs::write(dir.join(format!("{module}.py")), source)?;
    Ok(())
}

fn engine() -> Result<Engine> {
    Ok(Engine::new(
        &golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache(),
    )?)
}

/// One instance of a built component, called through `golem:agent/guest`.
struct Instance {
    store: Store<Host>,
    instance: wasmtime::component::Instance,
    guest: ComponentExportIndex,
}

impl Instance {
    async fn new(engine: &Engine, component: &Component) -> Result<Self> {
        let mut linker = Linker::<Host>::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        linker.define_unknown_imports_as_traps(component)?;
        let (ctx, io) = WasiCtxBuilder::new().build();
        let mut store = Store::new(
            engine,
            Host {
                ctx,
                io,
                table: ResourceTable::new(),
            },
        );
        store.set_fuel(u64::MAX)?;
        store.set_epoch_deadline(u64::MAX);
        let instance = linker.instantiate_async(&mut store, component).await?;
        let guest = instance
            .get_export_index(&mut store, None, "golem:agent/guest@2.0.0")
            .context("guest export")?;
        Ok(Self {
            store,
            instance,
            guest,
        })
    }

    async fn call(&mut self, name: &str, params: &[Val]) -> Result<Val> {
        let index = self
            .instance
            .get_export_index(&mut self.store, Some(&self.guest), name)
            .with_context(|| format!("{name} export"))?;
        let func = self
            .instance
            .get_func(&mut self.store, index)
            .with_context(|| format!("{name} function"))?;
        let mut results = [Val::Bool(false)];
        func.call_async(&mut self.store, params, &mut results)
            .await?;
        Ok(results[0].clone())
    }

    async fn initialize(&mut self, agent_type: &str, args: Vec<Val>) -> Result<Val> {
        self.call("initialize", &[Val::String(agent_type.into()), input(args)])
            .await
    }

    async fn invoke(&mut self, method: &str, args: Vec<Val>) -> Result<Val> {
        self.call("invoke", &[Val::String(method.into()), input(args)])
            .await
    }
}

async fn discover_agent_types(component: &[u8]) -> Result<Val> {
    let engine = engine()?;
    let component = Component::new(&engine, component)?;
    Instance::new(&engine, &component)
        .await?
        .call("discover-agent-types", &[])
        .await
}

fn node(case: &str, value: Val) -> Val {
    Val::Variant(case.into(), Some(Box::new(value)))
}

fn s64(value: i64) -> Val {
    node("s64-value", Val::S64(value))
}

fn string(value: &str) -> Val {
    node("string-value", Val::String(value.into()))
}

fn boolean(value: bool) -> Val {
    node("bool-value", Val::Bool(value))
}

fn binary(bytes: &[u8]) -> Val {
    node(
        "binary-value",
        Val::Record(vec![
            (
                "bytes".into(),
                Val::List(bytes.iter().copied().map(Val::U8).collect()),
            ),
            ("mime-type".into(), Val::Option(None)),
        ]),
    )
}

fn tree(nodes: Vec<Val>, root: i32) -> Val {
    Val::Record(vec![
        ("value-nodes".into(), Val::List(nodes)),
        ("root".into(), Val::S32(root)),
    ])
}

/// The value tree of a parameter list: a record whose fields are the
/// following nodes.
fn input(args: Vec<Val>) -> Val {
    let fields = (1..=args.len() as i32).map(Val::S32).collect();
    let mut nodes = vec![node("record-value", Val::List(fields))];
    nodes.extend(args);
    tree(nodes, 0)
}

/// The result of an invocation that returns `value`.
fn returned(value: Val) -> Val {
    Val::Result(Ok(Some(Box::new(Val::Option(Some(Box::new(tree(
        vec![value],
        0,
    ))))))))
}

fn unit_result() -> Val {
    Val::Result(Ok(Some(Box::new(Val::Option(None)))))
}

fn initialized() -> Val {
    Val::Result(Ok(None))
}

fn field<'a>(value: &'a Val, name: &str) -> &'a Val {
    match value {
        Val::Record(fields) => fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("no field {name} in {value:?}")),
        other => panic!("{other:?} is not a record"),
    }
}

fn list(value: &Val) -> &[Val] {
    match value {
        Val::List(items) => items,
        other => panic!("{other:?} is not a list"),
    }
}

/// The case and text of a failed call's agent error. A custom error carries
/// its message as a string value.
fn agent_error(result: &Val) -> (String, String) {
    let Val::Result(Err(Some(error))) = result else {
        panic!("{result:?} is not an agent error");
    };
    let Val::Variant(case, Some(payload)) = error.as_ref() else {
        panic!("{error:?} has no payload");
    };
    let text = match payload.as_ref() {
        Val::String(text) => text.clone(),
        typed => match &list(field(field(typed, "value"), "value-nodes"))[0] {
            Val::Variant(case, Some(text)) if case == "string-value" => match text.as_ref() {
                Val::String(text) => text.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?} is not a string value"),
        },
    };
    (case.clone(), text)
}

fn assert_agent_error(result: &Val, case: &str, text: &str) {
    let (actual_case, actual_text) = agent_error(result);
    assert_eq!(actual_case, case, "{actual_text}");
    assert!(actual_text.contains(text), "{actual_text:?} lacks {text:?}");
}

#[test]
async fn builds_unrelated_apps_from_the_same_prepared_assets() -> Result<()> {
    let prepared = prepared()?;
    let sdk = python_sdk().join("src");

    for (module, source) in [
        ("first_app", "VALUE = 1\n"),
        (
            "second_app",
            "import json\n\nVALUE = json.dumps({'a': [1, 2]})\n",
        ),
    ] {
        let dir = tempfile::tempdir()?;
        write_app(dir.path(), module, source)?;
        let component = build(
            &prepared,
            &Application {
                sdk_dir: &sdk,
                source_dir: dir.path(),
                entry_modules: &[module],
            },
        )
        .await?;

        assert_eq!(
            discover_agent_types(&component).await?,
            Val::Result(Ok(Some(Box::new(Val::List(vec![]))))),
            "{module} declares no agents"
        );
    }
    Ok(())
}

#[test]
async fn import_errors_keep_the_original_traceback() -> Result<()> {
    let prepared = prepared()?;
    let dir = tempfile::tempdir()?;
    write_app(
        dir.path(),
        "broken",
        "def load():\n    raise ValueError('cannot load settings')\n\nload()\n",
    )?;

    let error = build(
        &prepared,
        &Application {
            sdk_dir: &python_sdk().join("src"),
            source_dir: dir.path(),
            entry_modules: &["broken"],
        },
    )
    .await
    .expect_err("an entry module that raises fails the build");

    let message = format!("{error:#}");
    assert!(
        message.contains("ValueError: cannot load settings"),
        "{message}"
    );
    assert!(
        message.contains("broken.py\", line 2, in load"),
        "{message}"
    );
    Ok(())
}

#[test]
async fn invalid_entry_modules_fail_before_imports() -> Result<()> {
    let prepared = prepared()?;
    let dir = tempfile::tempdir()?;
    // Importing `marker` writes a file into the application directory.
    write_app(dir.path(), "marker", "open('/app/imported', 'w').close()\n")?;
    let sdk = python_sdk().join("src");

    let error = build(
        &prepared,
        &Application {
            sdk_dir: &sdk,
            source_dir: dir.path(),
            entry_modules: &["marker", "not a module"],
        },
    )
    .await
    .expect_err("an invalid entry module fails the build");
    assert!(
        format!("{error:#}").contains("`not a module` is not a Python module name"),
        "{error:#}"
    );
    assert!(!dir.path().join("imported").exists());

    build(
        &prepared,
        &Application {
            sdk_dir: &sdk,
            source_dir: dir.path(),
            entry_modules: &["marker"],
        },
    )
    .await?;
    assert!(
        dir.path().join("imported").exists(),
        "a valid build imports the entry module"
    );
    Ok(())
}

#[test]
async fn identical_inputs_give_identical_components() -> Result<()> {
    let prepared = prepared()?;
    let sdk = python_sdk().join("src");
    let source = "import json\n\nVALUE = json.dumps({'a': [1, 2]})\n";

    let mut components = Vec::new();
    // Each build uses a new application directory, and the assembler unpacks
    // the prepared assets into new directories. The application directories
    // stay alive until both builds finish, so the second one cannot reuse
    // the host inode of the first.
    let mut dirs = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir()?;
        write_app(dir.path(), "app", source)?;
        components.push(
            build(
                &prepared,
                &Application {
                    sdk_dir: &sdk,
                    source_dir: dir.path(),
                    entry_modules: &["app"],
                },
            )
            .await?,
        );
        dirs.push(dir);
    }
    let differing = components[0]
        .iter()
        .zip(&components[1])
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(components[0].len(), components[1].len());
    assert_eq!(differing, 0, "the two builds differ in {differing} bytes");
    Ok(())
}

const COUNTER: &str = r#"from golem import agent, method


@agent
class Counter:
    instances = 0

    def __init__(self, name: str, start: int) -> None:
        Counter.instances += 1
        self.name = name
        self.value = start

    @method
    def increment(self, amount: int) -> int:
        self.value += amount
        return self.value

    @method
    def describe(self, loud: bool, ratio: float, data: bytes) -> str:
        text = f"{self.name}={self.value} ratio={ratio} data={data.hex()}"
        return text.upper() if loud else text

    @method
    def reverse(self, data: bytes) -> bytes:
        return data[::-1]

    @method
    def positive(self) -> bool:
        return self.value > 0

    @method
    def reset(self) -> None:
        self.value = 0

    @method
    def constructed(self) -> int:
        return Counter.instances

    @method
    def bad_result(self) -> int:
        self.value += 1
        return True

    def helper(self) -> int:
        return 0
"#;

async fn build_counter() -> Result<Vec<u8>> {
    let dir = tempfile::tempdir()?;
    write_app(dir.path(), "counter", COUNTER)?;
    build(
        &prepared()?,
        &Application {
            sdk_dir: &python_sdk().join("src"),
            source_dir: dir.path(),
            entry_modules: &["counter"],
        },
    )
    .await
}

fn body_case(graph: &Val, index: &Val) -> String {
    let Val::S32(index) = index else {
        panic!("{index:?} is not a node index");
    };
    match field(&list(field(graph, "type-nodes"))[*index as usize], "body") {
        Val::Variant(case, _) => case.clone(),
        other => panic!("{other:?}"),
    }
}

fn parameters(graph: &Val, input_schema: &Val) -> Vec<(String, String)> {
    let Val::Variant(case, Some(fields)) = input_schema else {
        panic!("{input_schema:?}");
    };
    assert_eq!(case, "parameters");
    list(fields)
        .iter()
        .map(|parameter| {
            let Val::String(name) = field(parameter, "name") else {
                panic!("{parameter:?}");
            };
            (name.clone(), body_case(graph, field(parameter, "schema")))
        })
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(name, case)| (name.to_string(), case.to_string()))
        .collect()
}

#[test]
async fn discovery_describes_declared_agents_without_constructing_them() -> Result<()> {
    let component = build_counter().await?;
    let discovered = discover_agent_types(&component).await?;
    let Val::Result(Ok(Some(types))) = &discovered else {
        panic!("{discovered:?}");
    };
    let [counter] = list(types) else {
        panic!("{types:?}");
    };
    assert_eq!(field(counter, "type-name"), &Val::String("Counter".into()));
    assert_eq!(
        field(counter, "source-language"),
        &Val::String("python".into())
    );
    let graph = field(counter, "schema");
    assert_eq!(
        parameters(graph, field(field(counter, "constructor"), "input-schema")),
        pairs(&[("name", "string-type"), ("start", "s64-type")])
    );

    let methods: Vec<_> = list(field(counter, "methods"))
        .iter()
        .map(|method| {
            let Val::String(name) = field(method, "name") else {
                panic!("{method:?}");
            };
            let output = match field(method, "output-schema") {
                Val::Variant(case, None) if case == "unit" => "unit".to_string(),
                Val::Variant(case, Some(index)) if case == "single" => body_case(graph, index),
                other => panic!("{other:?}"),
            };
            (
                name.clone(),
                parameters(graph, field(method, "input-schema")),
                output,
            )
        })
        .collect();
    assert_eq!(
        methods,
        vec![
            (
                "increment".into(),
                pairs(&[("amount", "s64-type")]),
                "s64-type".into()
            ),
            (
                "describe".into(),
                pairs(&[
                    ("loud", "bool-type"),
                    ("ratio", "f64-type"),
                    ("data", "binary-type")
                ]),
                "string-type".into()
            ),
            (
                "reverse".into(),
                pairs(&[("data", "binary-type")]),
                "binary-type".into()
            ),
            ("positive".into(), vec![], "bool-type".into()),
            ("reset".into(), vec![], "unit".into()),
            ("constructed".into(), vec![], "s64-type".into()),
            ("bad_result".into(), vec![], "s64-type".into()),
        ]
    );

    // Neither pre-initialization nor discovery constructed an agent: the
    // first constructor call in this instance counts one.
    let engine = engine()?;
    let component = Component::new(&engine, &component)?;
    let mut agent = Instance::new(&engine, &component).await?;
    agent.call("discover-agent-types", &[]).await?;
    assert_eq!(
        agent
            .initialize("Counter", vec![string("alice"), s64(10)])
            .await?,
        initialized()
    );
    assert_eq!(agent.invoke("constructed", vec![]).await?, returned(s64(1)));
    let definition = agent.call("get-definition", &[]).await?;
    assert_eq!(
        field(&definition, "type-name"),
        &Val::String("Counter".into())
    );
    Ok(())
}

#[test]
async fn agents_keep_state_and_check_inputs_and_results() -> Result<()> {
    let engine = engine()?;
    let component = Component::new(&engine, build_counter().await?)?;

    let mut agent = Instance::new(&engine, &component).await?;
    assert_agent_error(
        &agent.invoke("increment", vec![s64(1)]).await?,
        "custom-error",
        "the agent is not initialized",
    );
    assert_agent_error(
        &agent.initialize("Missing", vec![]).await?,
        "invalid-type",
        "no agent type `Missing`; it defines `Counter`",
    );
    // A bool is not an int, and malformed input runs no constructor.
    assert_agent_error(
        &agent
            .initialize("Counter", vec![string("alice"), boolean(true)])
            .await?,
        "invalid-input",
        "parameter `start` expects a s64-value, got bool-value",
    );
    assert_eq!(
        agent
            .initialize("Counter", vec![string("alice"), s64(10)])
            .await?,
        initialized()
    );
    assert_eq!(agent.invoke("constructed", vec![]).await?, returned(s64(1)));

    assert_eq!(
        agent.invoke("increment", vec![s64(3)]).await?,
        returned(s64(13))
    );
    assert_eq!(
        agent.invoke("increment", vec![s64(7)]).await?,
        returned(s64(20))
    );

    // Initializing again fails and keeps the agent and its state.
    assert_agent_error(
        &agent
            .initialize("Counter", vec![string("bob"), s64(0)])
            .await?,
        "custom-error",
        "already initialized as `Counter`",
    );
    assert_eq!(agent.invoke("constructed", vec![]).await?, returned(s64(1)));

    // Malformed input fails before the method runs: the state is unchanged.
    for (args, message) in [
        (vec![boolean(true)], "expects a s64-value, got bool-value"),
        (vec![string("5")], "expects a s64-value, got string-value"),
        (vec![], "expected 1 parameters, got 0"),
        (vec![s64(1), s64(2)], "expected 1 parameters, got 2"),
    ] {
        assert_agent_error(
            &agent.invoke("increment", args).await?,
            "invalid-input",
            message,
        );
    }
    assert_eq!(
        agent.invoke("increment", vec![s64(0)]).await?,
        returned(s64(20))
    );
    assert_agent_error(
        &agent.invoke("helper", vec![]).await?,
        "invalid-method",
        "has no method `helper`",
    );

    assert_eq!(
        agent
            .invoke(
                "describe",
                vec![
                    boolean(true),
                    node("f64-value", Val::Float64(0.25)),
                    binary(&[0, 171])
                ]
            )
            .await?,
        returned(string("ALICE=20 RATIO=0.25 DATA=00AB"))
    );
    assert_eq!(
        agent.invoke("reverse", vec![binary(&[1, 2, 3])]).await?,
        returned(binary(&[3, 2, 1]))
    );
    assert_eq!(
        agent.invoke("positive", vec![]).await?,
        returned(boolean(true))
    );

    // A result of the wrong type fails after the method ran; its effect stays.
    assert_agent_error(
        &agent.invoke("bad_result", vec![]).await?,
        "custom-error",
        "`bad_result` returned an invalid result: expected int, got bool",
    );
    assert_eq!(
        agent.invoke("increment", vec![s64(0)]).await?,
        returned(s64(21))
    );
    assert_eq!(agent.invoke("reset", vec![]).await?, unit_result());
    assert_eq!(
        agent.invoke("positive", vec![]).await?,
        returned(boolean(false))
    );

    // Results must fit in a signed 64-bit integer.
    let mut high = Instance::new(&engine, &component).await?;
    high.initialize("Counter", vec![string("high"), s64(i64::MAX - 2)])
        .await?;
    assert_eq!(
        high.invoke("increment", vec![s64(2)]).await?,
        returned(s64(i64::MAX))
    );
    assert_agent_error(
        &high.invoke("increment", vec![s64(1)]).await?,
        "custom-error",
        "9223372036854775808 is out of the range of int",
    );
    // The failed call still added 1.
    assert_eq!(
        high.invoke("increment", vec![s64(-1)]).await?,
        returned(s64(i64::MAX))
    );

    let mut low = Instance::new(&engine, &component).await?;
    low.initialize("Counter", vec![string("low"), s64(i64::MIN)])
        .await?;
    assert_eq!(
        low.invoke("increment", vec![s64(0)]).await?,
        returned(s64(i64::MIN))
    );
    assert_agent_error(
        &low.invoke("increment", vec![s64(-1)]).await?,
        "custom-error",
        "-9223372036854775809 is out of the range of int",
    );
    Ok(())
}

#[test]
async fn invalid_declarations_fail_the_build() -> Result<()> {
    let dir = tempfile::tempdir()?;
    write_app(
        dir.path(),
        "shapes",
        r#"from typing import Annotated

from golem import agent, method


@agent
class Broken:
    config: 1 / 0


@agent
class Shapes:
    def __init__(self, items: list[int]) -> None:
        pass

    @method
    async def later(self) -> int:
        return 1

    @method
    def keyword(*, value: int) -> int:
        return value

    @method
    def wrapped(self, value: Annotated[int, "units"]) -> int:
        return value

    @method
    def unhashable(self, value: Annotated[int, {}]) -> Annotated[int, []]:
        return value


@agent
class Plain:
    def __init__(self, name: str) -> None:
        self.name = name

    @method
    def echo(self, value: int) -> int:
        return value
"#,
    )?;
    let error = build(
        &prepared()?,
        &Application {
            sdk_dir: &python_sdk().join("src"),
            source_dir: dir.path(),
            entry_modules: &["shapes"],
        },
    )
    .await
    .expect_err("invalid declarations fail the build");
    let message = format!("{error:#}");
    for expected in [
        "7 invalid agent declarations",
        "shapes.py:6: cannot evaluate the class annotations of agent `Broken`: ZeroDivisionError: division by zero",
        "shapes.py:13: parameter `items` of the constructor of `Shapes` has the unsupported type `list[int]`",
        "shapes.py:16: method `Shapes.later` is async",
        "shapes.py:20: method `Shapes.keyword` must take the agent instance as its first positional parameter",
        "shapes.py:24: parameter `value` of method `Shapes.wrapped` has the unsupported type `typing.Annotated[int, 'units']`",
        "shapes.py:28: parameter `value` of method `Shapes.unhashable` has the unsupported type `typing.Annotated[int, {}]`",
        "shapes.py:28: method `Shapes.unhashable` returns the unsupported type `typing.Annotated[int, []]`",
    ] {
        assert!(
            message.contains(expected),
            "{expected:?} is missing in {message}"
        );
    }
    assert!(!message.contains("Plain"), "{message}");
    Ok(())
}

//! Builds Python applications from the prepared SDK assets and runs them on
//! the shared engine.
//!
//! The prepared assets are a build output; `cargo make python-sdk-prepare`
//! writes them to `sdks/python/target/prepared`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use async_zip::{
    Compression, ZipEntryBuilder,
    tokio::{read::fs::ZipFileReader, write::ZipFileWriter},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use golem_python_assembler::{
    Application, COMPONENTIZE_PY_REVISION, SDK_VERSION, build, read_prepared, read_sdk_wheel,
};
use sha2::{Digest, Sha256};
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

/// The wheel that `cargo make python-sdk-wheel` builds.
fn sdk_wheel() -> PathBuf {
    python_sdk().join(format!(
        "target/wheel/golem_py_sdk-{SDK_VERSION}-py3-none-any.whl"
    ))
}

async fn wheel_files(wheel: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let reader = ZipFileReader::new(wheel).await.with_context(|| {
        format!(
            "run `cargo make python-sdk-wheel` to build {}",
            wheel.display()
        )
    })?;
    let mut files = Vec::new();
    for index in 0..reader.file().entries().len() {
        let name = reader.file().entries()[index]
            .filename()
            .as_str()?
            .to_string();
        let mut bytes = Vec::new();
        reader
            .reader_with_entry(index)
            .await?
            .read_to_end_checked(&mut bytes)
            .await?;
        files.push((name, bytes));
    }
    Ok(files)
}

async fn write_wheel(path: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    let mut writer = ZipFileWriter::with_tokio(tokio::fs::File::create(path).await?);
    for (name, bytes) in files {
        writer
            .write_entry_whole(
                ZipEntryBuilder::new(name.clone().into(), Compression::Deflate),
                bytes,
            )
            .await?;
    }
    writer.close().await?;
    Ok(())
}

/// Rewrites `RECORD` so that it lists the files as they are.
fn fix_record(files: &mut [(String, Vec<u8>)]) {
    let record = files
        .iter()
        .position(|(name, _)| name.ends_with(".dist-info/RECORD"))
        .expect("RECORD");
    let mut text = String::new();
    for (name, bytes) in files.iter() {
        if name.ends_with(".dist-info/RECORD") {
            text.push_str(&format!("{name},,\n"));
        } else {
            text.push_str(&format!(
                "{name},sha256={},{}\n",
                URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)),
                bytes.len()
            ));
        }
    }
    files[record].1 = text.into_bytes();
}

fn manifest_json(files: &mut [(String, Vec<u8>)]) -> serde_json::Value {
    serde_json::from_slice(file(files, "golem/_prepared/manifest.json")).unwrap()
}

/// Replaces the one occurrence of `from` in the prepared manifest.
fn edit_manifest(files: &mut [(String, Vec<u8>)], from: &str, to: &str) {
    let manifest = file(files, "golem/_prepared/manifest.json");
    let text = String::from_utf8(manifest.clone()).unwrap();
    assert_eq!(text.matches(from).count(), 1, "{from}");
    *manifest = text.replacen(from, to, 1).into_bytes();
}

fn remove_blob(files: &mut Vec<(String, Vec<u8>)>, blob: &str) {
    let name = format!("golem/_prepared/blobs/{blob}");
    let count = files.len();
    files.retain(|(file, _)| *file != name);
    assert_eq!(files.len(), count - 1, "{name}");
}

fn file<'a>(files: &'a mut [(String, Vec<u8>)], suffix: &str) -> &'a mut Vec<u8> {
    &mut files
        .iter_mut()
        .find(|(name, _)| name.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix}"))
        .1
}

#[test]
async fn builds_from_the_sdk_wheel() -> Result<()> {
    let sdk = read_sdk_wheel(&sdk_wheel()).await?;
    assert_eq!(sdk.prepared().identity(), prepared()?.identity());

    let dir = tempfile::tempdir()?;
    write_app(dir.path(), "counter", COUNTER)?;
    let from_wheel = build(
        sdk.prepared(),
        &Application {
            sdk_dir: sdk.sdk_dir(),
            source_dir: dir.path(),
            entry_modules: &["counter"],
        },
    )
    .await?;
    // The wheel holds the same SDK sources and prepared assets as the source
    // tree, so the component is the same.
    assert!(
        from_wheel == build_counter().await?,
        "the wheel and the source tree give different components"
    );

    let discovered = discover_agent_types(&from_wheel).await?;
    let Val::Result(Ok(Some(types))) = &discovered else {
        panic!("{discovered:?}");
    };
    let [counter] = list(types) else {
        panic!("{types:?}");
    };
    assert_eq!(field(counter, "type-name"), &Val::String("Counter".into()));
    Ok(())
}

#[test]
async fn rejects_mismatched_and_corrupt_wheels() -> Result<()> {
    let original = wheel_files(&sdk_wheel()).await?;
    type Edit = fn(&mut Vec<(String, Vec<u8>)>);
    let cases: [(&str, Edit, String); 8] = [
        (
            "corrupt SDK source",
            |files| file(files, "golem/_registry.py").push(b'#'),
            "`golem/_registry.py` does not match RECORD".to_string(),
        ),
        (
            "corrupt prepared blob, RECORD rewritten",
            |files| {
                let blob = files
                    .iter_mut()
                    .find(|(name, _)| name.contains("golem/_prepared/blobs/"))
                    .unwrap();
                blob.1[0] ^= 1;
                fix_record(files);
            },
            "prepared assets".to_string(),
        ),
        (
            "other SDK version",
            |files| {
                let metadata = file(files, ".dist-info/METADATA");
                let text = String::from_utf8(metadata.clone()).unwrap().replacen(
                    &format!("Version: {SDK_VERSION}"),
                    "Version: 9.9.9",
                    1,
                );
                *metadata = text.into_bytes();
                fix_record(files);
            },
            format!(
                "the wheel is golem-py-sdk 9.9.9, but this Golem CLI builds with golem-py-sdk {SDK_VERSION}"
            ),
        ),
        (
            "other componentize-py revision",
            |files| {
                *file(files, "golem/_compatibility.json") =
                    format!("{{\"componentize-py\": \"{}\"}}\n", "0".repeat(40)).into_bytes();
                fix_record(files);
            },
            format!(
                "comes from componentize-py {}, but this Golem CLI links componentize-py {COMPONENTIZE_PY_REVISION}",
                "0".repeat(40)
            ),
        ),
        (
            "no compatibility file",
            |files| {
                files.retain(|(name, _)| name != "golem/_compatibility.json");
                fix_record(files);
            },
            "the wheel has no `golem/_compatibility.json`".to_string(),
        ),
        (
            "file missing from RECORD",
            |files| files.push(("golem/extra.py".into(), b"X = 1\n".to_vec())),
            "RECORD does not list `golem/extra.py`".to_string(),
        ),
        (
            "file listed in RECORD but missing",
            |files| files.retain(|(name, _)| name != "golem/_dispatch.py"),
            "RECORD lists `golem/_dispatch.py`, which the wheel does not contain".to_string(),
        ),
        (
            "unsafe path",
            |files| {
                files.push(("golem/../../escape.py".into(), b"X = 1\n".to_vec()));
                fix_record(files);
            },
            "the wheel contains the unsafe path `golem/../../escape.py`".to_string(),
        ),
    ];
    let dir = tempfile::tempdir()?;
    for (case, edit, expected) in cases {
        let mut files = original.clone();
        edit(&mut files);
        let wheel = dir
            .path()
            .join(format!("golem_py_sdk-{SDK_VERSION}-py3-none-any.whl"));
        write_wheel(&wheel, &files).await?;
        let message = match read_sdk_wheel(&wheel).await {
            Ok(_) => panic!("{case}: the wheel was accepted"),
            Err(error) => format!("{error:#}"),
        };
        assert!(message.contains(&expected), "{case}: {message}");
        assert!(
            message.contains(&wheel.display().to_string()),
            "{case}: {message}"
        );
    }
    assert!(
        !dir.path().join("escape.py").exists()
            && !dir.path().parent().unwrap().join("escape.py").exists()
    );

    // Prepared assets from the same componentize-py revision, but without the
    // generated world bindings or with one runtime library fewer, are
    // internally valid. They are not the assets that the assembler builds
    // with, so both the wheel and the prepared directory are rejected.
    let expected = prepared()?.identity();
    let profiles: [(&str, Edit); 2] = [
        ("no world bindings", |files| {
            let world = manifest_json(files)["world"].as_str().unwrap().to_string();
            edit_manifest(files, &format!("\"world\": \"{world}\""), "\"world\": null");
            remove_blob(files, &world);
        }),
        ("one runtime library fewer", |files| {
            let library = "libwasi-emulated-getpid.so";
            let blob = manifest_json(files)["libraries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"] == library)
                .unwrap()["blob"]
                .as_str()
                .unwrap()
                .to_string();
            edit_manifest(
                files,
                &format!(
                    "    {{\n      \"name\": \"{library}\",\n      \"dl-openable\": false,\n      \"blob\": \"{blob}\"\n    }},\n"
                ),
                "",
            );
            remove_blob(files, &blob);
        }),
    ];
    for (index, (case, edit)) in profiles.into_iter().enumerate() {
        let mut files = original.clone();
        edit(&mut files);
        fix_record(&mut files);

        let prepared_dir = dir.path().join(format!("prepared-{index}"));
        for (name, bytes) in &files {
            if let Some(relative) = name.strip_prefix("golem/_prepared/") {
                let path = prepared_dir.join(relative);
                fs::create_dir_all(path.parent().unwrap())?;
                fs::write(path, bytes)?;
            }
        }
        let found = componentize_py::prepared::Prepared::read(&prepared_dir)
            .with_context(|| format!("{case}: the edited assets are not valid"))?
            .identity();
        assert_ne!(found, expected, "{case}");
        let mismatch =
            format!("the prepared assets are {found}, but this Golem CLI builds with {expected}");

        let wheel = dir
            .path()
            .join(format!("golem_py_sdk-{SDK_VERSION}-py3-none-any.whl"));
        write_wheel(&wheel, &files).await?;
        let message = match read_sdk_wheel(&wheel).await {
            Ok(_) => panic!("{case}: the wheel was accepted"),
            Err(error) => format!("{error:#}"),
        };
        assert!(message.contains(&mismatch), "{case}: {message}");
        let message = match read_prepared(&prepared_dir) {
            Ok(_) => panic!("{case}: the prepared directory was accepted"),
            Err(error) => format!("{error:#}"),
        };
        assert!(message.contains(&mismatch), "{case}: {message}");
    }

    // The unmodified files, rewritten the same way, are accepted.
    let wheel = dir.path().join("control.whl");
    write_wheel(&wheel, &original).await?;
    read_sdk_wheel(&wheel).await?;
    Ok(())
}

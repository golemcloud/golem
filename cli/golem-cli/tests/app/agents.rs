use crate::app::{cmd, flag, replace_strings_in_file, TestContext};
use crate::Tracing;
use golem_cli::fs;
use indoc::indoc;
use std::path::Path;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[test]
async fn test_rust_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "app:counter"])
        .await;
    assert!(outputs.success());

    let uuid = Uuid::new_v4().to_string();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("app:counter/counter-agent(\"{uuid}\")"),
            "increment",
        ])
        .await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains("- 1"));
}

#[test]
async fn test_rust_code_first_with_rpc_and_all_types() {
    let mut ctx = TestContext::new();

    let app_name = "rust-code-first";

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;

    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "rust", "rust:agent"])
        .await;

    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-rust")
            .join("rust-agent")
            .join("golem.yaml"),
    );

    let component_source_code_lib_file = ctx.cwd_path_join(
        Path::new("components-rust")
            .join("rust-agent")
            .join("src")
            .join("lib.rs"),
    );

    let component_source_code_model_file = ctx.cwd_path_join(
        Path::new("components-rust")
            .join("rust-agent")
            .join("src")
            .join("model.rs"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              rust:agent:
                templates: rust
        "# },
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("rust-code-first-snippets/lib.rs"),
        &component_source_code_lib_file,
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("rust-code-first-snippets/model.rs"),
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]).await;
    assert!(outputs.success());

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("rust:agent/foo-agent(some(\"{uuid}\"))");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success(), "function {func} failed");
    }

    run_and_assert(&ctx, "get-id", &[]).await;

    run_and_assert(&ctx, "rust:agent/foo-agent.{fun-string}", &["\"sample\""]).await;

    // A char type
    run_and_assert(&ctx, "fun-char", &[r#"'a'"#]).await;

    // Testing trigger invocation
    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-string-fire-and-forget}",
        &["\"sample\""],
    )
    .await;

    // Testing scheduled invocation
    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-string-later}",
        &["\"sample\""],
    )
    .await;

    run_and_assert(&ctx, "fun-u8", &["42"]).await;

    run_and_assert(&ctx, "fun-i8", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun-u16", &["42"]).await;

    run_and_assert(&ctx, "fun-i16", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun-i32", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun-u32", &["42"]).await;

    run_and_assert(&ctx, "fun-u64", &["42"]).await;

    run_and_assert(&ctx, "fun-i64", &["--", "-42"]).await;

    run_and_assert(&ctx, "fun-f32", &["3.14"]).await;

    run_and_assert(&ctx, "fun-f64", &["3.1415926535"]).await;

    run_and_assert(&ctx, "fun-boolean", &["true"]).await;

    let all_primitives_arg = r#"
    {
        u8v: 42,
        u16v: 42,
        u32v: 42,
        u64v: 42,
        i8v: -42,
        i16v: -42,
        i32v: -42,
        i64v: -42,
        f32v: 3.14,
        f64v: 3.1415926535,
        boolv: true,
        charv: 'a',
        stringv: "sample"
    }
    "#;

    run_and_assert(&ctx, "fun-all-primitives", &[all_primitives_arg]).await;

    run_and_assert(&ctx, "fun-tuple-simple", &[r#"("sample", 3.14, true)"#]).await;

    run_and_assert(
        &ctx,
        "fun-tuple-complex",
        &[&format!("(\"sample\", 3.14, {all_primitives_arg}, true)")],
    )
    .await;

    run_and_assert(
        &ctx,
        "fun-map",
        &[r#"[("foo", 1), ("bar", 2), ("baz", 3)]"#],
    )
    .await;

    let collections_arg = r#"
    {
        list-u8: [1, 2, 3, 4, 5],
        list-str: ["foo", "bar", "baz"],
        map-num: [("pi", 3.14), ("e", 2.71), ("phi", 1.61)],
        map-text: [(1, "one"), (2, "two"), (3, "three")]
    }
    "#;

    run_and_assert(&ctx, "fun-collections", &[collections_arg]).await;

    let simple_struct_arg = r#"
    {
        name: "test",
        value: 3.14,
        flag: true,
        symbol: 't',
    }
    "#;

    run_and_assert(&ctx, "fun-struct-simple", &[simple_struct_arg]).await;

    let nested_struct_arg = r#"
    {
        id: "nested1",
        simple: {
            name: "inner",
            value: 2.71,
            flag: false,
            symbol: 'i',
        },
        list: [
            {
                name: "list1",
                value: 1.61,
                flag: true,
                symbol: 'l',
            },
            {
                name: "list2",
                value: 0.577,
                flag: false,
                symbol: 'm',
            }
        ],
        map: [("a", 1), ("b", 2)],
        option: some("optional value"),
        result: ok("result value")
    }
    "#;

    run_and_assert(&ctx, "fun-struct-nested", &[nested_struct_arg]).await;

    let complex_struct_arg = r#"
    {
        primitives: {
            u8v: 1,
            u16v: 2,
            u32v: 3,
            u64v: 4,
            i8v: -1,
            i16v: -2,
            i32v: -3,
            i64v: -4,
            f32v: 1.1,
            f64v: 2.2,
            boolv: true,
            charv: 'c',
            stringv: "complex"
        },
        options-results-bounds: {
            option-u8: some(128),
            option-str: some("option value"),
            res-ok: ok("success"),
            res-num-err: err("number error"),
            res-unit-ok: ok("b"),
            res-unit-err: err("a"),
            bound-u8: included(1),
            bound-str: excluded("z")
        },
        tuples: {
            pair: ("pair", 2.22),
            triple: ("triple", 3.33, true),
            mixed: ( -8, 16, 4.4)
        },
        collections: {
            list-u8: [10, 20, 30],
            list-str: ["x", "y", "z"],
            map-num: [("a", 1.11), ("b", 2.22), ("c", 3.33)],
            map-text: [(100, "hundred"), (200, "two hundred"), (300, "three hundred")]
        },
        simple-struct: {
            name: "comp_simple",
            value: 5.55,
            flag: false,
            symbol: 's',
        },
        nested-struct: {
            id: "comp_nested",
            simple: {
                name: "comp_inner",
                value: 6.66,
                flag: true,
                symbol: 'i',
            },
            list: [],
            map: [],
            option: none,
            result: ok("nested result")
        },
        enum-simple: u8(100),
        enum-collections: vec([1, 2, 3]),
        enum-complex: unit-a
    }
    "#;

    run_and_assert(&ctx, "fun-struct-complex", &[complex_struct_arg]).await;

    run_and_assert(&ctx, "fun-simple-enum", &["i64(-12345)"]).await;

    // cli invoke gets confused with `fun-result` and `fun-result-unit-left` etc, and therefore fully qualified function name.
    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-result}",
        &["ok(\"success\")"],
    )
    .await;
    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-result}",
        &["err(\"failed\")"],
    )
    .await;

    run_and_assert(&ctx, "rust:agent/foo-agent.{fun-result-unit-ok}", &["ok"]).await;

    run_and_assert(&ctx, "rust:agent/foo-agent.{fun-result-unit-err}", &["err"]).await;

    run_and_assert(&ctx, "rust:agent/foo-agent.{fun-result-unit-both}", &["ok"]).await;

    let result_complex_arg = r#"
    ok({
        id: "res_comp",
        simple: {
            name: "res_inner",
            value: 7.77,
            flag: false,
            symbol: 'r',
        },
        list: [],
        map: [],
        option: none,
        result: ok("result in nested")
    })
    "#;

    run_and_assert(&ctx, "fun-result-complex", &[result_complex_arg]).await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-option}",
        &["some(\"optional value\")"],
    )
    .await;

    let option_complex_arg = r#"
    some({
        id: "opt_comp",
        simple: {
            name: "opt_inner",
            value: 8.88,
            flag: true,
            symbol: 'o',
        },
        list: [],
        map: [],
        option: none,
        result: err("error in nested")
    })
    "#;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-option-complex}",
        &[option_complex_arg],
    )
    .await;

    run_and_assert(&ctx, "fun-enum-with-only-literals", &["a"]).await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-multi-modal}",
        &[r#"[text("foo"), text("foo"), data({id: 1, name: "foo"})]"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-multi-modal-basic}",
        &[r#"[text(url("foo"))]"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-unstructured-text}",
        &[r#"url("foo")"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-unstructured-text}",
        &[r#"inline({data: "foo", text-type: none})"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-unstructured-text-lc}",
        &[r#"url("foo")"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-unstructured-text-lc}",
        &[r#"inline({data: "foo", text-type: some({language-code: "en"})})"#],
    )
    .await;

    run_and_assert(
        &ctx,
        "rust:agent/foo-agent.{fun-unstructured-binary}",
        &[r#"url("foo")"#],
    )
    .await;
}

#[test]
async fn test_ts_counter() {
    let mut ctx = TestContext::new();
    let app_name = "counter";

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter"])
        .await;
    assert!(outputs.success());

    let uuid = Uuid::new_v4().to_string();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &format!("app:counter/counter-agent(\"{uuid}\")"),
            "increment",
        ])
        .await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains("- 1"));
}

// Invocations on code-first typescript agents, with complex types / functions.
// Each function call is executed via RPC, and at every stage, mostly return type is same as input type.
// Early in the code-first release, some of these cases failed at the Golem execution stage
// (post type extraction). This test ensures such issues are caught automatically
// and act as a regression-test.
#[test]
async fn test_ts_code_first_with_rpc_and_all_types() {
    let mut ctx = TestContext::new();

    let app_name = "ts-code-first";

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;

    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "ts:agent"]).await;

    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("golem.yaml"),
    );

    let component_source_code_main_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("main.ts"),
    );

    let component_source_code_model_file = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("ts-agent")
            .join("src")
            .join("model.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              ts:agent:
                templates: ts
        "# },
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/main.ts"),
        &component_source_code_main_file,
    )
    .unwrap();

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/model.ts"),
        &component_source_code_model_file,
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success());

    async fn run_and_assert(ctx: &TestContext, func: &str, args: &[&str]) {
        let uuid = Uuid::new_v4().to_string();

        let agent_constructor = format!("ts:agent/foo-agent(\"{uuid}\")");

        let mut cmd = vec![flag::YES, cmd::AGENT, cmd::INVOKE, &agent_constructor, func];
        cmd.extend_from_slice(args);

        let outputs = ctx.cli(cmd).await;
        assert!(outputs.success(), "function {func} failed");
    }

    // fun with void return
    run_and_assert(&ctx, "fun-void-return", &["\"sample\""]).await;

    // fun without return type
    run_and_assert(&ctx, "fun-no-return", &["\"sample\""]).await;

    // function optional (that has null, defined as union)
    run_and_assert(
        &ctx,
        "ts:agent/foo-agent.{fun-optional}",
        &[
            "some(case1(\"foo\"))",
            "{a: some(\"foo\")}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(case1(\"foo\"))}",
            "{a: some(\"foo\")}",
            "some(\"foo\")",
            "some(case3(\"foo\"))",
        ],
    )
    .await;

    run_and_assert(&ctx, "fun-optional-q-mark", &["x", "none", r#"some("y")"#]).await;

    // function with a simple object
    run_and_assert(&ctx, "fun-object-type", &[r#"{a: "foo", b: 42, c: true}"#]).await;

    // function with a very complex object
    let argument = r#"
      {a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}
    "#;

    run_and_assert(&ctx, "fun-object-complex-type", &[argument]).await;

    // union type that has anonymous terms
    run_and_assert(&ctx, "fun-union-type", &[r#"union-type1("foo")"#]).await;

    // A complex union type
    run_and_assert(
        &ctx,
        "fun-union-complex-type",
        &[r#"union-complex-type1("foo")"#],
    )
    .await;

    // Union that includes literals and boolean (string literal input)
    run_and_assert(&ctx, "fun-union-with-literals", &[r#"lit1"#]).await;

    // Union that includes literals and boolean (boolean input)
    run_and_assert(
        &ctx,
        "fun-union-with-literals",
        &[r#"union-with-literals1(true)"#],
    )
    .await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["foo"]).await;

    // Unstructured text type
    run_and_assert(&ctx, "fun-unstructured-text", &["url(\"foo\")"]).await;

    // Unstructured binary
    run_and_assert(&ctx, "fun-unstructured-binary", &["url(\"foo\")"]).await;

    // Multimodal
    run_and_assert(&ctx, "fun-multimodal", &["[text(\"foo\")]"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["bar"]).await;

    // Union that has only literals
    run_and_assert(&ctx, "fun-union-with-only-literals", &["baz"]).await;

    // A number type
    run_and_assert(&ctx, "fun-number", &["42"]).await;

    // A string type
    run_and_assert(&ctx, "fun-string", &[r#""foo""#]).await;

    // A boolean type
    run_and_assert(&ctx, "fun-boolean", &["true"]).await;

    // A map type
    run_and_assert(&ctx, "fun-map", &[r#"[("foo", 42), ("bar", 42)]"#]).await;

    assert!(outputs.success());

    // A tagged union
    run_and_assert(&ctx, "fun-tagged-union", &[r#"a("foo")"#]).await;

    assert!(outputs.success());

    // A simple tuple type
    run_and_assert(&ctx, "fun-tuple-type", &[r#"("foo", 42, true)"#]).await;

    // A complex tuple type
    run_and_assert(
        &ctx,
        "fun-tuple-complex-type",
        &[r#"("foo", 42, {a: "foo", b: 42, c: true})"#],
    )
    .await;

    // A list complex type
    run_and_assert(
        &ctx,
        "fun-list-complex-type",
        &[r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#],
    ).await;

    // A function with null return
    run_and_assert(&ctx, "fun-null-return", &[r#""foo""#]).await;

    // A function with undefined return
    run_and_assert(&ctx, "fun-undefined-return", &[r#""foo""#]).await;

    // A function with result type
    run_and_assert(&ctx, "fun-result-exact", &[r#"ok("foo")"#]).await;

    // A function with (untagged) result-like type - but not result
    run_and_assert(
        &ctx,
        "fun-either-optional",
        &[r#"{ok: some("foo"), err: none}"#],
    )
    .await;

    // Functions using the builtin result type
    run_and_assert(&ctx, "fun-builtin-result-vs", &[r#"some("yay")"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-vs", &[r#"none"#]).await;

    run_and_assert(&ctx, "fun-builtin-result-sv", &[r#"none"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-sv", &[r#"some("yay")"#]).await;

    run_and_assert(&ctx, "fun-builtin-result-sn", &[r#"case1("yay")"#]).await;
    run_and_assert(&ctx, "fun-builtin-result-sn", &[r#"case2(42)"#]).await;

    // TODO: fix root cause for this
    // An arrow function
    // run_and_assert(&ctx, "fun-arrow-sync", &[r#""foo""#]).await;

    // A function that takes many inputs
    run_and_assert(
        &ctx,
        "fun-all",
        &[
            r#"{a: "foo", b: 42, c: true, d: {a: "foo", b: 42, c: true}, e: union-type1("foo"), f: ["foo", "foo", "foo"], g: [{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}], h: ("foo", 42, true), i: ("foo", 42, {a: "foo", b: 42, c: true}), j: [("foo", 42), ("foo", 42), ("foo", 42)], k: {n: 42}}"#,
            r#"union-type1("foo")"#,
            r#"union-complex-type1("foo")"#,
            r#"42"#,
            r#""foo""#,
            r#"true"#,
            r#"[("foo", 42), ("foo", 42), ("foo", 42)]"#,
            r#"("foo", 42, {a: "foo", b: 42, c: true})"#,
            r#"("foo", 42, true)"#,
            r#"[{a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}, {a: "foo", b: 42, c: true}]"#,
            r#"{a: "foo", b: 42, c: true}"#,
            r#"okay("foo")"#,
            r#"{ok: some("foo"), err: some("foo")}"#,
            r#"some(case1("foo"))"#,
            r#"{a: some("foo")}"#,
            r#"{a: some(case1("foo"))}"#,
            r#"{a: some(case1("foo"))}"#,
            r#"{a: some("foo")}"#,
            r#"some("foo")"#,
            r#"some(case3("foo"))"#,
            r#"a("foo")"#
        ],
    )
        .await;
}

#[test]
async fn test_common_dep_plugs_errors() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );
    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                templates: ts

                dependencies:
                - type: wasm
                  url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm
                - type: wasm
                  url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_google.wasm
                - type: wasm
                  url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_serper.wasm
                - type: wasm
                  url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_tavily.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(!outputs.success());
    assert2::assert!(outputs.stderr_contains_ordered(
        [
            "error: an error occurred when building the composition graph: multiple plugs found for export golem:web-search/types@1.0.0, only use one of them:",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_google.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_serper.wasm",
            "  - https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_tavily.wasm",
        ]
    ));

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                templates: ts

                dependencies:
                - type: wasm
                  url: https://github.com/golemcloud/golem-ai/releases/download/v0.3.0/golem_web_search_brave.wasm
        "# },
    )
        .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                templates: ts
        "# },
    )
    .unwrap();

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import * as websearch from 'golem:web-search/web-search@1.0.0';

            @agent()
            class Agent extends BaseAgent {
              async search(query: string): Promise<string> {
                let result = websearch.searchOnce({
                  query: query,
                });

                console.log(result);

                return "ok";
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "agent()",
            "search",
            "query",
        ])
        .await;
    assert!(!outputs.success());
    // If this fails, then adjust format_stack_line to match it
    assert!(outputs.stderr_contains("Library golem:web-search/web-search@1.0.0 called without being linked with an implementation"))
}

#[test]
async fn test_component_env_var_substitution() {
    let mut ctx = TestContext::new();
    let app_name = "env_var_substitution";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:weather-agent"])
        .await;
    assert!(outputs.success());

    let component_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-weather-agent")
            .join("golem.yaml"),
    );

    fs::write_str(
        &component_manifest_path,
        indoc! { r#"
            components:
              app:weather-agent:
                templates: ts
                env:
                  NORMAL: 'REALLY'
                  VERY_CUSTOM_ENV_VAR_SECRET_1: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}'
                  VERY_CUSTOM_ENV_VAR_SECRET_2: '{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
                  COMPOSED: '{{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}'
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    // Building is okay, as that does not resolve env vars
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    // But deploying will do so, so it should fail
    let outputs = ctx
        .cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY, flag::YES])
        .await;
    assert!(!outputs.success());

    assert!(outputs.stderr_contains_ordered([
        "key:       COMPOSED",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}-{{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_1",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_1 }}",
        "key:       VERY_CUSTOM_ENV_VAR_SECRET_2",
        "template:  {{ VERY_CUSTOM_ENV_VAR_SECRET_3 }}",
        "Failed to prepare environment variables for component: app:weather-agent",
    ]));

    // After providing the missing env vars, deploy should work
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_1", "123");
    ctx.add_env_var("VERY_CUSTOM_ENV_VAR_SECRET_3", "456");

    let outputs = ctx
        .cli([flag::SHOW_SENSITIVE, cmd::APP, cmd::DEPLOY, flag::YES])
        .await;
    assert!(outputs.success());

    assert!(outputs.stdout_contains_ordered([
        "+      env:",
        "+        COMPOSED: 123-456",
        "+        NORMAL: REALLY",
        "+        VERY_CUSTOM_ENV_VAR_SECRET_1: '123'",
        "+        VERY_CUSTOM_ENV_VAR_SECRET_2: '456'",
    ]));
}

#[test]
async fn test_http_api_merging() {
    let mut ctx = TestContext::new();
    let app_name = "http_api_merging";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter1"])
        .await;
    assert!(outputs.success());
    let component1_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter1")
            .join("golem.yaml"),
    );

    let outputs = ctx
        .cli([cmd::COMPONENT, cmd::NEW, "ts", "app:counter2"])
        .await;
    assert!(outputs.success());

    let component2_source_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter2")
            .join("src")
            .join("main.ts"),
    );
    replace_strings_in_file(component2_source_path, &[("CounterAgent", "CounterAgent2")]).unwrap();

    let component2_manifest_path = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-counter2")
            .join("golem.yaml"),
    );

    // Add mergeable definitions and deployments to both components
    fs::write_str(
        &component1_manifest_path,
        indoc! { r#"
            components:
              app:counter1:
                templates: ts

            httpApi:
              definitions:
                def-a:
                  version: 0.0.1
                  routes:
                  - method: GET
                    path: /a
                    binding:
                      componentName: app:counter1
                      response: |
                        let agent = counter-agent("a");
                        agent.increment()

              deployments:
                local:
                - domain: http_api_merging.localhost:9006
                  definitions:
                  - def-a
        "# },
    )
    .unwrap();

    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                templates: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent2("b");
                        agent.increment()

              deployments:
                local:
                - domain: http_api_merging.localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    // Check that the merged manifest is loadable
    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(!outputs.stdout_contains("error"));
    assert!(!outputs.stderr_contains("error"));
    assert!(outputs.stderr_contains_ordered([
        "Application API definitions:",
        "  def-a@0.0.1",
        "  def-b@0.0.2",
        "Application API deployments for environment local:",
        "  http_api_merging.localhost:9006",
        "    def-a",
        "    def-b",
    ]));

    // But we still cannot define the same deployment <-> definition in two places:
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter:
                templates: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter
                      response: |
                        let agent = counter-agent("b");
                        agent.increment()

              deployments:
                local:
                - domain: http_api_merging.localhost:9006
                  definitions:
                  - def-b
                  - def-a
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP]).await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains(
        "error: HTTP API Deployment local - http_api_merging.localhost:9006 - def-a is defined in multiple sources"
    ));

    // Let's switch back to the good config and deploy, then call the exposed APIs
    fs::write_str(
        &component2_manifest_path,
        indoc! { r#"
            components:
              app:counter2:
                templates: ts

            httpApi:
              definitions:
                def-b:
                  version: 0.0.2
                  routes:
                  - method: GET
                    path: /b
                    binding:
                      componentName: app:counter2
                      response: |
                        let agent = counter-agent2("b");
                        agent.increment()

              deployments:
                local:
                - domain: test_http_api_merging.localhost:9006
                  definitions:
                  - def-b
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        "+httpApiDeployments:",
        "+  http_api_merging.localhost:9006:",
        "+    apis:",
        "+    - def-a",
        "+  test_http_api_merging.localhost:9006:",
        "+    apis:",
        "+    - def-b",
        "Deployed all changes"
    ]));
}

#[test]
async fn test_invoke_and_repl_agent_id_casing_and_normalizing() {
    let mut ctx = TestContext::new();
    let app_name = "common_dep_plug_errors";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "app:agent"]).await;
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    let component_golem_yaml = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("golem.yaml"),
    );
    fs::write_str(
        &component_golem_yaml,
        indoc! { r#"
          components:
            app:agent:
              templates: ts
        "#},
    )
    .unwrap();

    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::write_str(
        &component_source_code,
        indoc! { r#"
            import { BaseAgent, agent, } from '@golemcloud/golem-ts-sdk';

            type Complex = {
              oneField: string;
              anotherField: number;
            }

            @agent()
            class LongAgentName extends BaseAgent {
              params: Complex;
              constructor(params: Complex) {
                super();
                this.params = params;
              }

              async ask(question: Complex): Promise<[Complex, Complex]> {
                return [this.params, question];
              }
            }
        "# },
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            r#"long-agent-name({one-field: "1212", another-field: 100})"#,
            "ask",
            r#"{one-field: "1", another-field: 2}"#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains_ordered([
        r#"long-agent-name({one-field:"1212",another-field:100})"#,
        r#"({one-field: "1212", another-field: 100}, {one-field: "1", another-field: 2})"#,
    ]));

    let outputs = ctx
        .cli([
            cmd::REPL,
            flag::FORMAT,
            "json",
            flag::SCRIPT,
            r#"
                let x = long-agent-name({one-field: "1212", another-field: 100});
                x.ask({one-field: "1", another-field: 2})
            "#,
        ])
        .await;
    assert!(outputs.success());
    assert!(outputs.stdout_contains(
        r#"{"another-field":100.0,"one-field":"1212"},{"another-field":2.0,"one-field":"1"}"#
    ));
}

#[test]
async fn test_naming_extremes() {
    let mut ctx = TestContext::new();
    let app_name = "test_naming_extremes";

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "ts"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "ts", "app:agent"]).await;
    assert!(outputs.success());

    let component_golem_yaml = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("golem.yaml"),
    );

    fs::write_str(
        component_golem_yaml,
        indoc! { r#"
            components:
              app:agent:
                templates: ts
        "# },
    )
    .unwrap();

    let component_source_code = ctx.cwd_path_join(
        Path::new("components-ts")
            .join("app-agent")
            .join("src")
            .join("main.ts"),
    );

    fs::copy(
        ctx.test_data_path_join("ts-code-first-snippets/naming_extremes.ts"),
        &component_source_code,
    )
    .unwrap();

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::INVOKE,
            flag::YES,
            r#"test-agent("x")"#,
            "test-all",
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!("string-agent(    \"{}\"    )", " ".repeat(447)), // HTTP API should normalize it
        ])
        .await;
    assert!(outputs.success());

    let outputs = ctx
        .cli([
            cmd::AGENT,
            cmd::GET,
            &format!(
                "struct-agent(  {{x:\"{}\"  ,  y    : \"{}\", z: \"{}\" }})", // HTTP API should normalize it
                " ".repeat(102),
                " ".repeat(102),
                "/".repeat(102)
            ),
        ])
        .await;
    assert!(outputs.success());
}

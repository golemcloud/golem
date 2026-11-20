use super::TestContext;
use golem_cli::{fs, model::invoke_result_view::InvokeResultView, versions};
use golem_common::schema::FromSchema;
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use test_r::{test, timeout};

async fn invoke(ctx: &TestContext, id: &str, method: &str, args: &[&str]) -> Value {
    let mut command = vec!["agent", "invoke", id, method];
    command.extend_from_slice(args);
    command.extend(["--no-stream", "--format", "json"]);
    let output = ctx.cli(command).await;
    assert!(output.success_or_dump());
    let documents: Vec<InvokeResultView> = output.stdout_json();
    let text = String::from_value(documents[0].result_json.as_ref().unwrap().value()).unwrap();
    let value = serde_json::from_str(&text).unwrap();
    println!("CONFIG {id} {method}: {value}");
    value
}

#[test]
#[timeout("20 minutes")]
async fn constructor_config_refreshes_on_update_and_replays_after_restart() {
    let mut ctx = TestContext::new();
    ctx.server_log = Some(ctx.cwd_path_join("server.log"));
    assert!(
        ctx.cli(["new", "moonbit-config", "--template", "moonbit", "--yes"])
            .await
            .success_or_dump()
    );
    ctx.cd("moonbit-config");
    let manifest = ctx.cwd_path_join("golem.yaml");
    let first_manifest = formatdoc! {r#"
        manifestVersion: {version}
        app: moonbit-config
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          moonbit-config:main:
            dir: .
            templates: moonbit
        config:
          local:
            label: first
        agents:
          ConfiguredAgent:
            config:
              label: {{ $config: label }}
    "#, version = versions::sdk::MANIFEST};
    fs::write_str(&manifest, &first_manifest).unwrap();
    let package = ctx.cwd_path_join("moon.pkg");
    fs::write_str(
        &package,
        fs::read_to_string(&package).unwrap().replacen(
            "import {",
            "import {\n  \"golemcloud/golem_sdk/config\" @config,",
            1,
        ),
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("config.mbt"), indoc! {r#"
        ///|
        #derive.golem_schema
        pub(all) struct Node {
          display_name : String
          next : Node?
        } derive(ToJson)

        ///|
        #derive.config
        pub(all) struct Settings {
          label : String
          #derive.config(name="maxChars", default="4000")
          max_chars : UInt32
          #derive.config(default="{\"displayName\":\"outer\",\"next\":{\"displayName\":\"inner\",\"next\":null}}")
          tree : Node
          #derive.config(default="\"not-cleared\"")
          cleared : String?
          #derive.config(default="true")
          enabled : Bool
          #derive.config(default="[\"baseline\"]")
          values : Array[String]
        }

        ///|
        #derive.agent
        struct ConfiguredAgent {
          config : @config.Config[Settings]
          labels : Array[String]
        }

        ///|
        fn ConfiguredAgent::new(_name : String, config : @config.Config[Settings]) -> ConfiguredAgent {
          { config, labels: [], }
        }

        ///|
        pub fn ConfiguredAgent::read(self : Self) -> String {
          let c = self.config.value
          let child : Json = match c.tree.next {
            Some(node) => node.display_name.to_json()
            None => Json::null()
          }
          let cleared : Json = match c.cleared {
            Some(value) => value.to_json()
            None => Json::null()
          }
          ({ "label": c.label.to_json(), "maxChars": c.max_chars.to_json(),
             "root": c.tree.display_name.to_json(), "child": child,
             "cleared": cleared, "enabled": c.enabled.to_json(),
             "values": c.values.to_json() } : Json).stringify()
        }

        ///|
        pub fn ConfiguredAgent::remember(self : Self) -> String {
          self.labels.push(self.config.value.label)
          self.labels.to_json().stringify()
        }

        ///|
        #derive.agent
        struct ConfigCaller {
          name : String
          mut target : @schema.AgentId?
        }

        ///|
        fn ConfigCaller::new(name : String) -> ConfigCaller { { name, target: None, } }

        ///|
        pub async fn ConfigCaller::run(self : Self, root : String, child : String, limit : UInt32) -> String {
          let client = match self.target {
            Some(id) => ConfiguredAgentClient::get_by_id(id)
            None => {
              let client = ConfiguredAgentClient::create("rpc-created", keyed=true, config={
                label: None,
                max_chars: Some(limit),
                tree: Some({ display_name: root, next: Some({ display_name: child, next: None, }), }),
                cleared: Some(None),
                enabled: Some(false),
                values: Some([]),
              })
              self.target = Some(client.agent_id())
              client
            }
          }
          defer client.drop()
          client.read()
        }
    "#}).unwrap();
    assert!(ctx.cli(["build", "--yes"]).await.success_or_dump());
    ctx.start_server().await;
    assert!(ctx.cli(["deploy", "--yes"]).await.success_or_dump());
    let defaults = json!({"label":"first", "maxChars":4000, "root":"outer", "child":"inner",
        "cleared":"not-cleared", "enabled":true, "values":["baseline"]});
    let target = r#"ConfiguredAgent("rpc-created")"#;
    let ordinary = r#"ConfiguredAgent("defaults")"#;
    assert_eq!(invoke(&ctx, ordinary, "read", &[]).await, defaults);
    let captured = json!({"label":"first", "maxChars":0, "root":"parent-before", "child":"child-before",
        "cleared":null, "enabled":false, "values":[]});
    assert_eq!(
        invoke(
            &ctx,
            r#"ConfigCaller("before")"#,
            "run",
            &["parent-before", "child-before", "0"]
        )
        .await,
        captured
    );
    assert_eq!(invoke(&ctx, target, "read", &[]).await, captured);
    assert_eq!(
        invoke(&ctx, target, "remember", &[]).await,
        json!(["first"])
    );

    fs::write_str(
        &manifest,
        first_manifest.replace("label: first", "label: second"),
    )
    .unwrap();
    assert!(ctx.cli(["deploy", "--yes"]).await.success_or_dump());
    assert_eq!(invoke(&ctx, ordinary, "read", &[]).await, defaults);
    let mut new_defaults = defaults;
    new_defaults["label"] = json!("second");
    assert_eq!(
        invoke(&ctx, r#"ConfiguredAgent("new")"#, "read", &[]).await,
        new_defaults
    );
    for id in [target, ordinary] {
        assert!(
            ctx.cli(["agent", "update", id, "--await", "--yes"])
                .await
                .success_or_dump()
        );
    }
    let mut updated = captured;
    updated["label"] = json!("second");
    assert_eq!(invoke(&ctx, target, "read", &[]).await, updated);
    assert_eq!(invoke(&ctx, ordinary, "read", &[]).await, new_defaults);
    assert_eq!(
        invoke(&ctx, target, "remember", &[]).await,
        json!(["first", "second"])
    );

    println!(
        "STOPPING SERVER PID {:?}",
        ctx.server_process.as_ref().unwrap().id()
    );
    ctx.stop_server().await;
    assert!(ctx.server_process.is_none());
    ctx.start_server().await;
    println!(
        "RESTARTED SERVER PID {:?}",
        ctx.server_process.as_ref().unwrap().id()
    );
    assert_eq!(
        invoke(
            &ctx,
            r#"ConfigCaller("before")"#,
            "run",
            &["ignored-after", "different-child", "99"]
        )
        .await,
        updated
    );
    assert_eq!(invoke(&ctx, target, "read", &[]).await, updated);
    assert_eq!(invoke(&ctx, ordinary, "read", &[]).await, new_defaults);
    assert_eq!(
        invoke(&ctx, target, "remember", &[]).await,
        json!(["first", "second", "second"])
    );
    ctx.stop_server().await;
}

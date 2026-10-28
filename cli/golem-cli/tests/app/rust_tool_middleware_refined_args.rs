// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

const SLUG_DEFINITION: &str = indoc! { r#"
    #[golem_rust::tool_definition(version = "1.0.0")]
    pub trait Slug {
        #[arg(name = "positional", min_length = 2, max_length = 8)]
        fn slug(&self, name: String) -> String;

        #[arg(count = "positional", max = 3)]
        fn limit(&self, count: u32) -> u32;
    }
"# };

/// A Rust middleware sits in front of a Rust tool whose parameters carry text
/// and numeric refinements. The middleware reads each refined argument as its
/// Rust parameter type and forwards it to the tool, which publishes the text
/// argument as `text`. The caller sends the published input record directly,
/// so a value that breaks a refinement reaches the host validation, which must
/// name the broken restriction.
#[test]
#[timeout("20 minutes")]
async fn rust_tool_middleware_forwards_refined_arguments() {
    let mut ctx = TestContext::new();
    let app_name = "refined-middleware";

    ctx.start_server().await;

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();
    ctx.cd(app_name);

    for component_name in [
        "refined-middleware:provider",
        "refined-middleware:middleware",
        "refined-middleware:consumer",
    ] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "rust",
                flag::COMPONENT_NAME,
                component_name,
            ])
            .await;
        assert!(outputs.success_or_dump());
    }

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: refined-middleware

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              refined-middleware:provider:
                dir: "provider"
                templates: rust
              refined-middleware:middleware:
                dir: "middleware"
                templates: rust
              refined-middleware:consumer:
                dir: "consumer"
                templates: rust

            tools:
              slug:
                component: refined-middleware:provider
              middleware:
                slug-audit:
                  component: refined-middleware:middleware

            agents:
              SlugConsumerAgent:
                tools:
                  slug:
                    middleware:
                      - name: slug-audit
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("provider/src/counter_agent.rs"),
        formatdoc! { r#"
            {SLUG_DEFINITION}
            struct SlugImpl;

            #[golem_rust::tool_implementation]
            impl Slug for SlugImpl {{
                fn slug(&self, name: String) -> String {{
                    format!("slug:{{name}}")
                }}

                fn limit(&self, count: u32) -> u32 {{
                    count * 10
                }}
            }}
        "# },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("middleware/src/counter_agent.rs"),
        formatdoc! { r#"
            use golem_rust::{{tool::ToolInvokeError, tool_middleware}};
            use std::convert::Infallible;

            {SLUG_DEFINITION}
            struct SlugAudit;

            impl SlugAudit {{
                fn new() -> Self {{
                    Self
                }}
            }}

            #[tool_middleware(name = "slug-audit", constructor = SlugAudit::new)]
            impl SlugMiddleware for SlugAudit {{
                async fn slug(
                    &self,
                    underlying: &SlugUnderlying,
                    name: String,
                ) -> Result<String, ToolInvokeError<Infallible>> {{
                    underlying.slug(name).await.map(|value| format!("audited-{{value}}"))
                }}

                async fn limit(
                    &self,
                    underlying: &SlugUnderlying,
                    count: u32,
                ) -> Result<u32, ToolInvokeError<Infallible>> {{
                    underlying.limit(count).await.map(|value| value + 1)
                }}
            }}
        "# },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("consumer/src/counter_agent.rs"),
        formatdoc! { r#"
            use golem_rust::agentic::{{DynamicToolClient, ToolBuildCtx}};
            use golem_rust::schema::TextValuePayload;
            use golem_rust::{{SchemaValue, TypedSchemaValue, agent_definition, agent_implementation}};

            #[allow(dead_code)]
            mod slug_definition {{
                {SLUG_DEFINITION}
            }}

            fn text(value: &str) -> SchemaValue {{
                SchemaValue::Text(TextValuePayload {{
                    text: value.to_string(),
                    language: None,
                }})
            }}

            async fn invoke(command_path: &[&str], argument: SchemaValue) -> String {{
                let tool = slug_definition::__golem_tool_descriptor_for_Slug(&mut ToolBuildCtx::new())
                    .expect("tool descriptor builds");
                let path: Vec<String> = command_path.iter().map(|segment| segment.to_string()).collect();
                let index = tool.command_index_by_path(&path).expect("command exists");
                let model = tool.canonical_input_model(index).expect("input model builds");
                let input = TypedSchemaValue::new(
                    model.record_schema,
                    SchemaValue::Record {{ fields: vec![argument] }},
                );
                match DynamicToolClient::new("slug").invoke(&path, &input).await {{
                    Ok(result) => match result.result.map(|value| value.into_parts().1) {{
                        Some(SchemaValue::String(value)) => format!("ok:{{value}}"),
                        Some(SchemaValue::U32(value)) => format!("ok:{{value}}"),
                        other => format!("unexpected:{{other:?}}"),
                    }},
                    Err(error) => format!("err:{{error}}"),
                }}
            }}

            #[agent_definition]
            pub trait SlugConsumerAgent {{
                fn new(name: String) -> Self;
                async fn call_all(&self) -> Vec<String>;
            }}

            struct SlugConsumerImpl;

            #[agent_implementation]
            impl SlugConsumerAgent for SlugConsumerImpl {{
                fn new(_name: String) -> Self {{
                    Self
                }}

                async fn call_all(&self) -> Vec<String> {{
                    let mut results = Vec::new();
                    for name in ["golem", "toolongslug", "g"] {{
                        // `slug` has the name of the tool, so it is the root command.
                        let result = invoke(&[], text(name)).await;
                        results.push(format!("slug-{{name}}={{result}}"));
                    }}
                    for count in [2, 9] {{
                        let result = invoke(&["limit"], SchemaValue::U32(count)).await;
                        results.push(format!("limit-{{count}}={{result}}"));
                    }}
                    results
                }}
            }}
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let agent_constructor = format!("SlugConsumerAgent(\"{}\")", Uuid::new_v4());
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            &agent_constructor,
            "call_all",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let stdout = outputs.stdout().collect::<Vec<_>>().join("\n");
    let result = |prefix: &str| {
        let start = stdout
            .find(prefix)
            .unwrap_or_else(|| panic!("missing {prefix} in:\n{stdout}"));
        let rest = &stdout[start..];
        let end = rest
            .match_indices('"')
            .find(|(index, _)| !rest[..*index].ends_with('\\'))
            .map_or(rest.len(), |(index, _)| index);
        rest[..end].to_string()
    };

    assert_eq!(
        result("slug-golem="),
        "slug-golem=ok:audited-slug:golem",
        "a valid text-refined argument must pass the middleware and reach the provider:\n{stdout}"
    );
    let too_long = result("slug-toolongslug=");
    assert!(
        too_long.starts_with("slug-toolongslug=err:")
            && too_long.contains("invalid input")
            && too_long.contains("above max-length 8"),
        "an argument above max_length must fail with a length validation error:\n{stdout}"
    );
    let too_short = result("slug-g=");
    assert!(
        too_short.starts_with("slug-g=err:")
            && too_short.contains("invalid input")
            && too_short.contains("below min-length 2"),
        "an argument below min_length must fail with a length validation error:\n{stdout}"
    );
    assert_eq!(
        result("limit-2="),
        "limit-2=ok:21",
        "a valid numeric-refined argument must pass the middleware and reach the provider:\n{stdout}"
    );
    let above_max = result("limit-9=");
    assert!(
        above_max.starts_with("limit-9=err:")
            && above_max.contains("invalid input")
            && above_max.contains("above maximum 3"),
        "an argument above the numeric max must fail with a validation error:\n{stdout}"
    );
}

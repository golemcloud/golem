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
        #[arg(name = "positional", max_length = 8, regex = "^[a-z]+$")]
        fn slug(&self, name: String) -> String;

        #[arg(count = "positional", max = 3)]
        fn limit(&self, count: u32) -> u32;
    }
"# };

/// A Rust caller sends a `String` argument through the same-language tool
/// client to a Rust tool whose parameter carries text refinements. The tool
/// publishes that parameter as `text`, so the client must send a `text` value.
/// A value that breaks the refinement must fail with a validation error that
/// names the broken restriction. A numeric refinement follows the same rule.
#[test]
#[timeout("20 minutes")]
async fn rust_tool_client_sends_text_refined_string_argument() {
    let mut ctx = TestContext::new();
    let app_name = "refined-args";

    ctx.start_server().await;

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();
    ctx.cd(app_name);

    for component_name in ["refined-args:provider", "refined-args:consumer"] {
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

            app: refined-args

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              refined-args:provider:
                dir: "provider"
                templates: rust
              refined-args:consumer:
                dir: "consumer"
                templates: rust

            tools:
              slug:
                component: refined-args:provider

            agents:
              SlugConsumerAgent:
                tools:
                  slug: {{}}
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
        ctx.cwd_path_join("consumer/src/counter_agent.rs"),
        formatdoc! { r#"
            use golem_rust::{{agent_definition, agent_implementation}};

            #[allow(dead_code)]
            mod slug_definition {{
                {SLUG_DEFINITION}
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
                    let client = slug_definition::SlugClient::new();
                    let mut results = Vec::new();
                    for name in ["golem", "toolongslug", "Golem"] {{
                        results.push(match client.slug(name.to_string()).await {{
                            Ok(value) => format!("slug-{{name}}=ok:{{value}}"),
                            Err(error) => format!("slug-{{name}}=err:{{error}}"),
                        }});
                    }}
                    for count in [2, 9] {{
                        results.push(match client.limit(count).await {{
                            Ok(value) => format!("limit-{{count}}=ok:{{value}}"),
                            Err(error) => format!("limit-{{count}}=err:{{error}}"),
                        }});
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
        "slug-golem=ok:slug:golem",
        "a valid text-refined argument must reach the provider:\n{stdout}"
    );
    let too_long = result("slug-toolongslug=");
    assert!(
        too_long.starts_with("slug-toolongslug=err:")
            && too_long.contains("invalid input")
            && too_long.contains("above max-length 8"),
        "an argument above max_length must fail with a length validation error:\n{stdout}"
    );
    let not_matching = result("slug-Golem=");
    assert!(
        not_matching.starts_with("slug-Golem=err:")
            && not_matching.contains("invalid input")
            && not_matching.contains("does not match required regex"),
        "an argument that breaks the regex must fail with a regex validation error:\n{stdout}"
    );
    assert_eq!(
        result("limit-2="),
        "limit-2=ok:20",
        "a valid numeric-refined argument must reach the provider:\n{stdout}"
    );
    let above_max = result("limit-9=");
    assert!(
        above_max.starts_with("limit-9=err:") && above_max.contains("invalid input"),
        "an argument above the numeric max must fail with a validation error:\n{stdout}"
    );
}

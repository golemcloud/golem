// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::{app_manifest_version, GOLEM_AI_SUFFIX, GOLEM_AI_VERSION};
use indoc::formatdoc;
use std::sync::LazyLock;

pub struct DocDependencyGroup {
    pub name: &'static str,
    pub dependencies: Vec<DocDependency>,
}

pub struct DocDependency {
    pub name: &'static str,
    pub env_vars: Vec<DocDependencyEnvVar>,
    #[allow(unused)]
    pub url: String,
}

pub struct DocDependencyEnvVar {
    pub name: &'static str,
    pub value: &'static str,
    pub comment: &'static str,
}

pub static APP_MANIFEST_HEADER: LazyLock<String> = LazyLock::new(|| {
    formatdoc! {"
            # Schema for IDEA:
            # $schema: https://schema.golem.cloud/app/golem/{version}/golem.schema.json
            # Schema for vscode-yaml:
            # yaml-language-server: $schema=https://schema.golem.cloud/app/golem/{version}/golem.schema.json

            # Field reference: https://learn.golem.cloud/app-manifest#field-reference
            # Creating HTTP APIs: https://learn.golem.cloud/invoke/making-custom-apis
        ",
        version = app_manifest_version!()
    }
});

pub static DOC_DEPENDENCIES: LazyLock<Vec<DocDependencyGroup>> = LazyLock::new(|| {
    fn golem_ai(name: &str) -> String {
        format!(
            "https://github.com/golemcloud/golem-ai/releases/download/{}/{}{}",
            GOLEM_AI_VERSION, name, GOLEM_AI_SUFFIX
        )
    }

    fn env(name: &'static str, value: &'static str, comment: &'static str) -> DocDependencyEnvVar {
        DocDependencyEnvVar {
            name,
            value,
            comment,
        }
    }

    fn dep(name: &'static str, env_vars: Vec<DocDependencyEnvVar>, url: String) -> DocDependency {
        DocDependency {
            name,
            env_vars,
            url,
        }
    }

    fn dep_group(name: &'static str, dependencies: Vec<DocDependency>) -> DocDependencyGroup {
        DocDependencyGroup { name, dependencies }
    }

    vec![
        dep_group(
            "LLM providers",
            vec![
                dep(
                    "Common",
                    vec![env("GOLEM_LLM_LOG", "trace", "Optional, defaults to warn")],
                    "".to_string(),
                ),
                dep(
                    "Anthropic",
                    vec![env("ANTHROPIC_API_KEY", "<KEY>", "")],
                    golem_ai("golem_llm_anthropic"),
                ),
                dep(
                    "OpenAI",
                    vec![env("OPENAI_API_KEY", "<KEY>", "")],
                    golem_ai("golem_llm_openai"),
                ),
                dep(
                    "OpenRouter",
                    vec![env("OPENROUTER_API_KEY", "<KEY>", "")],
                    golem_ai("golem_llm_openrouter"),
                ),
                dep(
                    "Amazon Bedrock",
                    vec![
                        env("AWS_ACCESS_KEY_ID", "<KEY>", ""),
                        env("AWS_REGION", "<REGION>", ""),
                        env("AWS_SECRET_ACCESS_KEY", "<KEY>", ""),
                        env("AWS_SESSION_TOKEN", "<TOKEN>", "Optional"),
                    ],
                    golem_ai("golem_llm_bedrock"),
                ),
                dep(
                    "Grok",
                    vec![env("XAI_API_KEY", "<KEY>", "")],
                    golem_ai("golem_llm_grok"),
                ),
                dep(
                    "Ollama",
                    vec![env(
                        "GOLEM_OLLAMA_BASE_URL",
                        "<URL>",
                        "Optional, defaults to http://localhost:11434",
                    )],
                    golem_ai("golem_llm_ollama"),
                ),
            ],
        ),
        dep_group(
            "Code execution providers",
            vec![
                dep("Python and JavaScript", vec![], golem_ai("golem_exec")),
                dep("Python only", vec![], golem_ai("golem_exec_python")),
                dep("JavaScript only", vec![], golem_ai("golem_exec_javascript")),
            ],
        ),
        dep_group(
            "Embedding providers",
            vec![
                dep(
                    "OpenAI",
                    vec![env("OPENAI_API_KEY", "<KEY>", "")],
                    golem_ai("golem_embed_openai"),
                ),
                dep(
                    "Cohere",
                    vec![env("COHERE_API_KEY", "<KEY>", "")],
                    golem_ai("golem_embed_cohere"),
                ),
                dep(
                    "HuggingFace",
                    vec![env("HUGGING_FACE_API_KEY", "<KEY>", "")],
                    golem_ai("golem_embed_hugging_face"),
                ),
                dep(
                    "VoyageAI",
                    vec![env("VOYAGEAI_API_KEY", "<KEY>", "")],
                    golem_ai("golem_embed_voyageai"),
                ),
            ],
        ),
        dep_group(
            "Graph database providers",
            vec![
                dep(
                    "ArangoDB",
                    vec![
                        env("ARANGODB_HOST", "<HOST>", ""),
                        env("ARANGODB_PORT", "<PORT>", "Optional, defaults to 8529"),
                        env("ARANGODB_USER", "<USER>", ""),
                        env("ARANGODB_PASSWORD", "<PASS>", ""),
                        env("ARANGO_DATABASE", "<DB>", ""),
                    ],
                    golem_ai("golem_graph_arangodb"),
                ),
                dep(
                    "JanusGraph",
                    vec![
                        env("JANUSGRAPH_HOST", "<HOST>", ""),
                        env("JANUSGRAPH_PORT", "<PORT>", "Optional, defaults to 8182"),
                        env("JANUSGRAPH_USER", "<USER>", ""),
                        env("JANUSGRAPH_PASSWORD", "<PASS>", ""),
                    ],
                    golem_ai("golem_graph_janusgraph"),
                ),
                dep(
                    "Neo4j",
                    vec![
                        env("NEO4J_HOST", "<HOST>", ""),
                        env("NEO4J_PORT", "<PORT>", "Optional, defaults to 7687"),
                        env("NEO4J_USER", "<USER>", ""),
                        env("NEO4J_PASSWORD", "<PASS>", ""),
                    ],
                    golem_ai("golem_graph_neo4j"),
                ),
            ],
        ),
        dep_group(
            "Search providers",
            vec![
                dep(
                    "Common",
                    vec![env(
                        "GOLEM_SEARCH_LOG",
                        "trace",
                        "Optional, defaults to warn",
                    )],
                    "".to_string(),
                ),
                dep(
                    "Algolia",
                    vec![
                        env("ALGOLIA_APPLICATION_ID", "<ID>", ""),
                        env("ALGOLIA_API_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_search_algolia"),
                ),
                dep(
                    "ElasticSearch",
                    vec![
                        env("ELASTICSEARCH_URL", "<URL>", ""),
                        env("ELASTICSEARCH_USERNAME", "<USERNAME>", ""),
                        env("ELASTICSEARCH_PASSWORD", "<PASSWORD>", ""),
                        env("ELASTICSEARCH_API_KEY", "<API_KEY>", ""),
                    ],
                    golem_ai("golem_search_elasticsearch"),
                ),
                dep(
                    "Meilisearch",
                    vec![
                        env("MEILISEARCH_BASE_URL", "<URL>", ""),
                        env("MEILISEARCH_API_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_search_meilisearch"),
                ),
                dep(
                    "OpenSearch",
                    vec![
                        env("OPENSEARCH_BASE_URL", "<URL>", ""),
                        env("OPENSEARCH_USERNAME", "<USER>", ""),
                        env("OPENSEARCH_PASSWORD", "<PASS>", ""),
                        env("OPENSEARCH_API_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_search_opensearch"),
                ),
                dep(
                    "Typesense",
                    vec![
                        env("TYPESENSE_BASE_URL", "<URL>", ""),
                        env("TYPESENSE_API_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_search_typesense"),
                ),
            ],
        ),
        dep_group(
            "Speech-to-text providers",
            vec![
                dep(
                    "Common",
                    vec![
                        env(
                            "STT_PROVIDER_LOG_LEVEL",
                            "trace",
                            "Optional, defaults to warn",
                        ),
                        env("STT_PROVIDER_MAX_RETRIES", "10", "Optional, defaults to 10"),
                    ],
                    "".to_string(),
                ),
                dep(
                    "AWS",
                    vec![
                        env("AWS_REGION", "<REGION>", ""),
                        env("AWS_ACCESS_KEY", "<KEY>", ""),
                        env("AWS_SECRET_KEY", "<KEY>", ""),
                        env("AWS_BUCKET_NAME", "<BUCKET>", ""),
                    ],
                    golem_ai("golem_stt_aws"),
                ),
                dep(
                    "Azure",
                    vec![
                        env("AZURE_REGION", "<REGION>", ""),
                        env("AZURE_SUBSCRIPTION_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_stt_azure"),
                ),
                dep(
                    "Deepgram",
                    vec![
                        env("DEEPGRAM_API_TOKEN", "<TOKEN>", ""),
                        env("DEEPGRAM_ENDPOINT", "<URL>", "Optional"),
                    ],
                    golem_ai("golem_stt_deepgram"),
                ),
                dep(
                    "Google",
                    vec![
                        env("GOOGLE_LOCATION", "", ""),
                        env("GOOGLE_BUCKET_NAME", "", ""),
                        env(
                            "GOOGLE_APPLICATION_CREDENTIALS",
                            "<CRED>",
                            "or use the vars below",
                        ),
                        env("GOOGLE_PROJECT_ID", "<ID>", ""),
                        env("GOOGLE_CLIENT_EMAIL", "<EMAIL>", ""),
                        env("GOOGLE_PRIVATE_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_stt_google"),
                ),
                dep(
                    "Whisper",
                    vec![env("OPENAI_API_KEY", "<KEY>", "")],
                    golem_ai("golem_stt_whisper"),
                ),
            ],
        ),
        dep_group(
            "Video generation providers",
            vec![
                dep(
                    "Kling",
                    vec![
                        env("KLING_ACCESS_KEY", "<KEY>", ""),
                        env("KLING_SECRET_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_video_kling"),
                ),
                dep(
                    "Runway",
                    vec![env("RUNWAY_API_KEY", "<KEY>", "")],
                    golem_ai("golem_video_runway"),
                ),
                dep(
                    "Stability",
                    vec![env("STABILITY_API_KEY", "<KEY>", "")],
                    golem_ai("golem_video_stability"),
                ),
                dep(
                    "Veo",
                    vec![
                        env("VEO_PROJECT_ID", "<ID>", ""),
                        env("VEO_CLIENT_EMAIL", "<EMAIL>", ""),
                        env("VEO_PRIVATE_KEY", "<KEY>", ""),
                    ],
                    golem_ai("golem_video_veo"),
                ),
            ],
        ),
        dep_group(
            "WebSearch providers",
            vec![
                dep(
                    "Brave",
                    vec![env("BRAVE_API_KEY", "<KEY>", "")],
                    golem_ai("golem_web_search_brave"),
                ),
                dep(
                    "Google",
                    vec![
                        env("GOOGLE_API_KEY", "<KEY>", ""),
                        env("GOOGLE_SEARCH_ENGINE_ID", "<ID>", ""),
                    ],
                    golem_ai("golem_web_search_google"),
                ),
                dep(
                    "Serper",
                    vec![env("SERPER_API_KEY", "<KEY>", "")],
                    golem_ai("golem_web_search_serper"),
                ),
                dep(
                    "Tavily",
                    vec![env("TAVILY_API_KEY", "<KEY>", "")],
                    golem_ai("golem_web_search_tavily"),
                ),
            ],
        ),
    ]
});

pub static DEP_ENV_VARS_DOC: LazyLock<String> = LazyLock::new(|| {
    let indent = "    ";
    let mut out = String::new();

    for group in DOC_DEPENDENCIES.iter() {
        if !group
            .dependencies
            .iter()
            .any(|dep| !dep.env_vars.is_empty())
        {
            continue;
        }

        out.push_str(&doc_group_header(indent, group));

        for dep in &group.dependencies {
            if dep.env_vars.is_empty() {
                continue;
            }

            out.push_str(&doc_dep_header(indent, dep));

            for v in &dep.env_vars {
                let mut line = format!("{indent}# {key}", indent = indent, key = v.name);
                line.push_str(&format!(": \"{}\"", v.value));
                if !v.comment.is_empty() {
                    line.push_str(&format!(" # {}", v.comment));
                }
                line.push('\n');
                out.push_str(&line);
            }

            out.push('\n');
        }

        out.push('\n');
    }

    out.trim_end().to_string()
});

fn doc_group_header(indent: &str, group: &DocDependencyGroup) -> String {
    format!(
        "{indent}# {name}\n{indent}# {decor}\n\n",
        indent = indent,
        name = group.name,
        decor = "-".repeat(indent.len() + group.name.len() - 4)
    )
}

fn doc_dep_header(indent: &str, dep: &DocDependency) -> String {
    format!("{indent}## {name}\n", indent = indent, name = dep.name)
}

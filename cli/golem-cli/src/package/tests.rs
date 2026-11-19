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

use super::*;
use golem_common::model::agent::{AgentConfigSource, AgentMode, Snapshotting};
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, AgentConstructorSchema, AgentTypeKind, AgentTypeSchema,
    InputSchema,
};
use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
use golem_common::schema::{SchemaGraph, SchemaType};
use golem_schema::Empty;
use serde_json::json;
use test_r::test;

fn sample() -> (PackageConfig, BTreeMap<String, Vec<u8>>) {
    let mut layers = BTreeMap::new();
    let mut components = BTreeMap::new();
    for (name, byte) in [("first", 1), ("second", 2)] {
        let wasm = add_blob(&mut layers, WASM, vec![byte]);
        components.insert(
            ComponentName(name.into()),
            PackageComponent {
                wasm: wasm.digest,
                metadata: ExtractedComponentMetadata {
                    agent_types: vec![AgentTypeSchema {
                        kind: AgentTypeKind::Regular,
                        type_name: AgentTypeName(format!("{name}Agent")),
                        description: "An example agent".into(),
                        source_language: "rust".into(),
                        schema: SchemaGraph::empty(),
                        constructor: AgentConstructorSchema {
                            name: None,
                            description: String::new(),
                            prompt_hint: None,
                            input_schema: InputSchema::parameters(vec![]),
                        },
                        methods: vec![],
                        dependencies: vec![],
                        mode: AgentMode::Ephemeral,
                        http_mount: None,
                        snapshotting: Snapshotting::Disabled(Empty {}),
                        config: vec![],
                    }],
                    tools: vec![Tool {
                        version: "1.0.0".into(),
                        requires_filesystem: false,
                        http_mount: None,
                        schema: SchemaGraph::empty(),
                        commands: CommandTree {
                            nodes: vec![CommandNode {
                                name: format!("{name}-tool"),
                                aliases: vec![],
                                doc: Doc {
                                    summary: format!("Run {name}"),
                                    description: String::new(),
                                    examples: vec![],
                                },
                                http: None,
                                globals: Globals::default(),
                                subcommands: vec![],
                                body: None,
                            }],
                        },
                    }],
                    tool_middlewares: vec![],
                },
                config_schema: Default::default(),
                provision: PackageProvision {
                    config: json!({"setting": name}),
                    ..Default::default()
                },
                agents: Default::default(),
                tools: Default::default(),
                tool_middlewares: Default::default(),
            },
        );
    }
    (
        PackageConfig {
            application: "example".into(),
            components,
        },
        layers,
    )
}

#[test]
fn archive_round_trips_and_content_determines_identity() {
    let (mut config, mut layers) = sample();
    let (digest, archive) = pack(&config, &layers).unwrap();
    assert_eq!(
        pack(&config, &layers).unwrap(),
        (digest.clone(), archive.clone())
    );
    let validated = read(&archive[..]).unwrap();
    assert_eq!(validated.digest(), digest);
    assert_eq!(
        serde_json::to_value(validated.config()).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
    for (name, component) in &validated.config().components {
        assert_eq!(
            component.metadata.agent_types[0].type_name.0,
            format!("{name}Agent")
        );
        assert_eq!(
            component.metadata.tools[0].name(),
            Some(format!("{name}-tool").as_str())
        );
    }

    let first = config.components.values_mut().next().unwrap();
    first.metadata.agent_types[0].description.push('!');
    assert_ne!(pack(&config, &layers).unwrap().0, digest);
    config
        .components
        .values_mut()
        .next()
        .unwrap()
        .metadata
        .agent_types[0]
        .description
        .pop();

    let first = config
        .components
        .get_mut(&ComponentName("first".into()))
        .unwrap();
    let original = layers.remove(&first.wasm).unwrap();
    first.wasm = add_blob(&mut layers, WASM, vec![original[0] ^ 1]).digest;
    let replacement_digest = pack(&config, &layers).unwrap().0;
    assert_ne!(replacement_digest, digest);
}

#[test]
fn refuses_secret_paths_in_typed_defaults() {
    use golem_common::schema::schema_type::SecretSpec;
    let (mut config, layers) = sample();
    let component = config.components.values_mut().next().unwrap();
    component.config_schema.declarations.extend([
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Secret,
            path: vec!["nested".into(), "credential".into()],
            value_type: SchemaType::string(),
        },
        AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["nested".into(), "records".into()],
            value_type: SchemaType::list(SchemaType::record_from_fields([(
                "credential",
                SchemaType::secret(SecretSpec::default()),
            )])),
        },
    ]);
    for value in [
        json!({"nested":{"credential":"private-key"}}),
        json!({"nested":{"records":[{"credential":"private-key"}]}}),
    ] {
        config
            .components
            .values_mut()
            .next()
            .unwrap()
            .provision
            .config = value;
        let error = format!("{:#}", pack(&config, &layers).unwrap_err());
        assert!(error.contains("secret"), "{error}");
        assert!(!error.contains("private-key"), "{error}");
    }
    config
        .components
        .values_mut()
        .next()
        .unwrap()
        .provision
        .config = json!({"nested":{"records":[]}});
    assert!(pack(&config, &layers).is_ok());
}

#[test]
fn only_the_matching_union_branch_contributes_file_defaults() {
    use golem_common::schema::schema_type::{
        DiscriminatorRule, FieldDiscriminator, UnionBranch, UnionSpec,
    };
    use golem_schema::{IntoSchema, SchemaBuilder};

    let file = golem_schema::schema::File::register_in(&mut SchemaBuilder::new());
    let declaration = AgentConfigDeclarationSchema {
        source: AgentConfigSource::Local,
        path: vec!["input".into()],
        value_type: SchemaType::union(UnionSpec {
            branches: [("file", file), ("text", SchemaType::string())]
                .into_iter()
                .map(|(tag, ty)| UnionBranch {
                    tag: tag.into(),
                    body: SchemaType::record_from_fields([
                        ("kind", SchemaType::string()),
                        ("source", ty),
                    ]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".into(),
                        literal: Some(tag.into()),
                    }),
                    metadata: Default::default(),
                })
                .collect(),
        }),
    };
    for (kind, expected) in [
        (
            "file",
            BTreeSet::from([vec!["input".into(), "source".into()]]),
        ),
        ("text", BTreeSet::new()),
    ] {
        assert_eq!(
            file_default_paths(
                &json!({"input": {"kind": kind, "source": "./sample.txt"}}),
                &SchemaGraph::empty(),
                std::slice::from_ref(&declaration),
            )
            .unwrap(),
            expected,
        );
    }
}

#[test]
fn refuses_secret_and_config_defaults_at_every_depth() {
    for key in ["$secret", "$config"] {
        let reference = json!({ (key): "private-key" });
        for value in [
            reference.clone(),
            json!({"record": [0, {"field": reference}]}),
        ] {
            let (mut config, layers) = sample();
            config
                .components
                .values_mut()
                .next()
                .unwrap()
                .provision
                .config = value;
            let error = format!("{:#}", pack(&config, &layers).unwrap_err());
            assert!(error.contains(key), "{error}");
            assert!(!error.contains("private-key"), "{error}");
        }
    }
}

fn tar_with(path: &str, kind: tar::EntryType, link: &str, repeat: bool) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(kind);
    header.set_size(0);
    // Set the raw name so the fixture can express paths the writer would reject.
    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
    if !link.is_empty() {
        header.set_link_name_literal(link.as_bytes()).unwrap();
    }
    header.set_cksum();
    tar.append(&header, &[][..]).unwrap();
    if repeat {
        tar.append(&header, &[][..]).unwrap();
    }
    tar.into_inner().unwrap()
}

#[test]
fn refuses_unsafe_or_duplicate_entries_and_missing_index() {
    for (path, kind, link, repeat) in [
        ("/escape", tar::EntryType::Regular, "", false),
        ("blobs/../escape", tar::EntryType::Regular, "", false),
        ("index.json", tar::EntryType::Symlink, "../outside", false),
        ("index.json", tar::EntryType::Link, "../outside", false),
        ("index.json", tar::EntryType::Regular, "", true),
        ("oci-layout", tar::EntryType::Regular, "", false),
    ] {
        let name = if path == "oci-layout" {
            "index.json"
        } else {
            path
        };
        let error = format!(
            "{:#}",
            read(&tar_with(path, kind, link, repeat)[..]).unwrap_err()
        );
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn refuses_a_layer_whose_bytes_do_not_match_its_digest() {
    let (config, layers) = sample();
    let (_, archive) = pack(&config, &layers).unwrap();
    let mut entries = archive_entries(&archive[..]).unwrap();
    let path = blob_path(&config.components.values().next().unwrap().wasm).unwrap();
    entries.get_mut(&path).unwrap()[0] ^= 1;
    let broken = write_tar(&entries).unwrap();
    let error = format!("{:#}", read(&broken[..]).unwrap_err());
    assert!(error.contains(&path), "{error}");
}

#[test]
fn refuses_noncanonical_json_with_duplicate_keys() {
    let (config, layers) = sample();
    let (_, archive) = pack(&config, &layers).unwrap();
    let mut entries = archive_entries(&archive[..]).unwrap();
    entries.insert(
        "oci-layout".into(),
        br#"{"imageLayoutVersion":"0.0.0","imageLayoutVersion":"1.0.0"}"#.to_vec(),
    );
    let broken = write_tar(&entries).unwrap();
    let error = format!("{:#}", read(&broken[..]).unwrap_err());
    assert!(error.contains("oci-layout"), "{error}");
}

#[test]
fn refuses_broken_initial_file_sets() {
    let (mut config, mut blobs) = sample();
    let valid_layer = write_tar(&BTreeMap::from([
        ("00000000".into(), b"first"),
        ("00000001".into(), b"other"),
    ]))
    .unwrap();
    for (layer, target, expected) in [
        (valid_layer, "/first", "Duplicate initial-file target"),
        (
            b"not a tar".to_vec(),
            "/other",
            "Invalid initial-file layer",
        ),
    ] {
        let descriptor = add_blob(&mut blobs, INITIAL_FILES, layer);
        config
            .components
            .values_mut()
            .next()
            .unwrap()
            .provision
            .files = Some(PackageInitialFiles {
            layer: descriptor.digest.clone(),
            entries: ["/first", target]
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    (
                        format!("{index:08}"),
                        PackageInitialFile {
                            target: AgentFilePath::from_abs_str(path).unwrap(),
                            permissions: AgentFilePermissions::ReadOnly,
                        },
                    )
                })
                .collect(),
        });
        // The writer runs the reader on what it produced, including if the blob digest is intact.
        let error = format!("{:#}", pack(&config, &blobs).unwrap_err());
        assert!(error.contains(expected), "{error}");
        assert!(
            error.contains(&blob_path(&descriptor.digest).unwrap()),
            "{error}"
        );
    }
}

#[test]
fn refuses_duplicate_middleware_exports_even_without_provisions() {
    use golem_common::schema::tool::{ToolMiddleware, ToolMiddlewareScope};
    let (mut config, blobs) = sample();
    for component in config.components.values_mut() {
        component.metadata.tool_middlewares.push(ToolMiddleware {
            name: "example-middleware".into(),
            version: "1.0.0".into(),
            aliases: vec![],
            doc: Doc {
                summary: "Example middleware".into(),
                description: String::new(),
                examples: vec![],
            },
            scope: ToolMiddlewareScope::Universal,
            parameter_schema: SchemaGraph::empty(),
        });
    }
    let result = pack(&config, &blobs);
    assert!(
        matches!(result, Err(error) if error.to_string().contains("Duplicate tool middleware name or alias example-middleware"))
    );
}

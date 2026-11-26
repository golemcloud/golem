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
use std::path::{Path, PathBuf};
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
            package: PackageIdentity {
                name: "example".into(),
                version: "1.2.3".into(),
                calls: BTreeSet::new(),
            },
            components,
        },
        layers,
    )
}

#[test]
fn lock_checks_missing_extra_version_source_and_digest() {
    let manifest = Pins::from([(
        "example".into(),
        FilePin {
            source: "example.tar".into(),
            version: "1.2.3".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
        },
    )]);
    assert!(check_lock(&manifest, &manifest).is_ok());
    let missing = check_lock(&manifest, &Pins::new()).unwrap_err();
    assert!(matches!(missing, LockError::Missing(_)));
    let extra = check_lock(&Pins::new(), &manifest).unwrap_err();
    assert!(matches!(extra, LockError::Extra(_)));
    assert!(
        extra
            .to_string()
            .contains("delete this entry from golem.lock")
    );
    for err in [missing, extra] {
        assert!(err.to_string().contains("golem install"));
    }
    for field in ["version", "source", "digest"] {
        let mut lock = manifest.clone();
        let pin = lock.get_mut("example").unwrap();
        match field {
            "version" => pin.version = "2.0.0".into(),
            "source" => pin.source = "other.tar".into(),
            _ => pin.digest = format!("sha256:{}", "b".repeat(64)),
        }
        let err = check_lock(&manifest, &lock).unwrap_err();
        assert!(matches!(err, LockError::Mismatch(_, f) if f == field));
        assert!(err.to_string().contains("golem install"));
    }
}

#[derive(Default)]
struct MemoryEffects {
    cache: BTreeMap<String, Vec<u8>>,
    source: Option<Vec<u8>>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    deny_source: bool,
}

#[async_trait::async_trait]
impl PackageEffects for MemoryEffects {
    fn authorize(&self, _: &str, _: &FilePin) -> anyhow::Result<()> {
        ensure!(!self.deny_source, "Source denied");
        Ok(())
    }
    async fn cached(&mut self, digest: &str, _: u64) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.cache.get(digest).cloned())
    }
    async fn fetch(&mut self, _: &FilePin, _: u64) -> anyhow::Result<Vec<u8>> {
        self.source.take().context("Source is unavailable")
    }
    async fn cache(&mut self, digest: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.cache.insert(digest.into(), bytes.into());
        Ok(())
    }
    async fn create_dir(&mut self, root: &Path, relative: &Path) -> anyhow::Result<()> {
        assert_eq!(root, Path::new("unpack"));
        assert!(
            relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        );
        Ok(())
    }
    async fn write_file(
        &mut self,
        root: &Path,
        relative: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        self.create_dir(root, relative).await?;
        assert!(
            self.files
                .insert(relative.to_owned(), bytes.into())
                .is_none()
        );
        Ok(())
    }
}

#[test]
fn package_rejects_blank_calls_and_provisions_for_unexported_agents() {
    let (mut spec, blobs) = sample();
    spec.package.calls.insert(AgentTypeName(" ".into()));
    assert!(
        pack(&spec, &blobs)
            .unwrap_err()
            .to_string()
            .contains("empty agent type name")
    );
    spec.package.calls.clear();
    spec.components.values_mut().next().unwrap().agents.insert(
        AgentTypeName("MissingAgent".into()),
        PackageProvision::default(),
    );
    assert!(
        pack(&spec, &blobs)
            .unwrap_err()
            .to_string()
            .contains("MissingAgent has no matching exported agent type")
    );
}

#[test]
async fn expansion_rejects_duplicate_exported_agent_types_with_both_packages_named() {
    let (mut spec, blobs) = sample();
    assert!(
        spec.components
            .values()
            .all(|component| component.agents.is_empty())
    );
    let mut effects = MemoryEffects::default();
    let mut pins = Pins::new();
    for name in ["first-package", "second-package"] {
        spec.package.name = name.into();
        let (digest, bytes) = pack(&spec, &blobs).unwrap();
        effects.cache.insert(digest.clone(), bytes);
        pins.insert(
            name.into(),
            FilePin {
                source: format!("{name}.tar"),
                version: spec.package.version.clone(),
                digest,
            },
        );
    }
    let error = expand(&pins, &pins, Path::new("unpack"), &mut effects)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Duplicate installed agent type firstAgent in packages \"first-package\" and \"second-package\""), "{error}");
}

#[test]
async fn expansion_verifies_cache_and_materializes_ordinary_declarations() {
    let (mut spec, blobs) = sample();
    spec.package
        .calls
        .insert(AgentTypeName("SupportCall".into()));
    let (digest, bytes) = pack(&spec, &blobs).unwrap();
    spec.package.version = "3.2.1".into();
    let (_, wrong) = pack(&spec, &blobs).unwrap();
    let pin = FilePin {
        source: "one.tar".into(),
        version: "1.2.3".into(),
        digest: digest.clone(),
    };
    let pins = Pins::from([("example".into(), pin)]);
    let root = Path::new("unpack");
    let mut effects = MemoryEffects {
        cache: BTreeMap::from([(digest.clone(), wrong)]),
        source: Some(bytes.clone()),
        ..Default::default()
    };
    let expanded = expand(&pins, &pins, root, &mut effects).await.unwrap();
    assert_eq!(
        expanded.calls["example"],
        BTreeSet::from([AgentTypeName("SupportCall".into())])
    );
    assert_eq!(effects.cache[&digest], bytes);
    assert!(effects.source.is_none());
    let declaration = serde_json::to_value(expanded.declarations).unwrap();
    for (name, byte) in [("first", 1), ("second", 2)] {
        let component = &declaration["components"][name];
        assert_eq!(component["config"], json!({"setting": name}));
        let path = Path::new(component["componentWasm"].as_str().unwrap());
        assert!(path.starts_with("example/wasm"));
        assert_eq!(effects.files[path], [byte]);
    }
    effects.files.clear();
    assert!(expand(&pins, &pins, root, &mut effects).await.is_ok());
    effects.files.clear();
    effects.deny_source = true;
    assert!(
        expand(&pins, &pins, root, &mut effects)
            .await
            .unwrap_err()
            .to_string()
            .contains("Source denied")
    );
    assert!(effects.files.is_empty());
}

#[test]
async fn expansion_refuses_a_replacement_at_the_locked_url() {
    let (spec, mut blobs) = sample();
    let (digest, _) = pack(&spec, &blobs).unwrap();
    let mut replacement = spec;
    replacement.components.values_mut().next().unwrap().wasm =
        add_blob(&mut blobs, WASM, vec![9]).digest;
    let (replacement_digest, bytes) = pack(&replacement, &blobs).unwrap();
    assert_ne!(digest, replacement_digest);
    let pins = Pins::from([(
        "example".into(),
        FilePin {
            source: "https://example.com/package.tar".into(),
            version: "1.2.3".into(),
            digest,
        },
    )]);
    let mut effects = MemoryEffects {
        source: Some(bytes),
        ..Default::default()
    };
    let error = expand(&pins, &pins, Path::new("unpack"), &mut effects)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Package digest mismatch") && error.contains(&replacement_digest),
        "{error}"
    );
    assert!(effects.cache.is_empty() && effects.files.is_empty());
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
fn rejects_secret_defaults_only_in_the_selected_union_branch() {
    use golem_common::schema::schema_type::{
        DiscriminatorRule, FieldDiscriminator, SecretSpec, UnionBranch, UnionSpec,
    };

    let (mut config, layers) = sample();
    let component = config.components.values_mut().next().unwrap();
    component
        .config_schema
        .declarations
        .push(AgentConfigDeclarationSchema {
            source: AgentConfigSource::Local,
            path: vec!["input".into()],
            value_type: SchemaType::union(UnionSpec {
                branches: [
                    ("public", SchemaType::string()),
                    ("private", SchemaType::secret(SecretSpec::default())),
                ]
                .into_iter()
                .map(|(tag, ty)| UnionBranch {
                    tag: tag.into(),
                    body: SchemaType::record_from_fields([
                        ("kind", SchemaType::string()),
                        ("value", ty),
                    ]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".into(),
                        literal: Some(tag.into()),
                    }),
                    metadata: Default::default(),
                })
                .collect(),
            }),
        });
    for (kind, accepted) in [("private", false), ("public", true)] {
        config
            .components
            .values_mut()
            .next()
            .unwrap()
            .provision
            .config = json!({"input": {"kind": kind, "value": "ordinary text"}});
        match pack(&config, &layers) {
            Ok(_) => assert!(accepted, "accepted a secret default"),
            Err(err) => {
                assert!(!accepted, "public default rejected: {err:#}");
                assert!(format!("{err:#}").contains("supplies a secret at input"));
            }
        }
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
    let digest = format!("sha256:{:x}", Sha256::digest(b"manifest"));
    for (path, kind, link, repeat) in [
        ("/escape", tar::EntryType::Regular, "", false),
        ("blobs/../escape", tar::EntryType::Regular, "", false),
        ("blobs/.. /escape", tar::EntryType::Regular, "", false),
        ("blobs/NUL.txt", tar::EntryType::Regular, "", false),
        ("index.json", tar::EntryType::Symlink, "../outside", false),
        ("index.json", tar::EntryType::Link, "../outside", false),
        ("index.json", tar::EntryType::Regular, "", true),
        ("oci-layout", tar::EntryType::Regular, "", false),
    ] {
        let name = if repeat {
            "Duplicate tar entry"
        } else if path == "oci-layout" {
            "index.json"
        } else {
            path
        };
        let tar = tar_with(path, kind, link, repeat);
        let error = format!("{:#}", read(&tar[..]).unwrap_err());
        assert!(error.contains(name), "{error}");
        let error = verify_manifest_digest(std::io::Cursor::new(&tar), &digest).unwrap_err();
        assert!(format!("{error:#}").contains(name));
    }
}

#[test]
fn rejects_inconsistent_or_unreferenced_oci_layers() {
    let (config, layers) = sample();
    let (digest, archive) = pack(&config, &layers).unwrap();
    let original = archive_entries(archive.as_slice()).unwrap();
    for expected in [
        "Unexpected package entry",
        "Size mismatch",
        "Layer count mismatch",
    ] {
        let mut entries = original.clone();
        let mut digest = digest.clone();
        if expected == "Unexpected package entry" {
            let bytes = b"unreferenced";
            entries.insert(
                blob_path(&describe(WASM, bytes).digest).unwrap(),
                bytes.to_vec(),
            );
        } else {
            let mut index: OciIndex = parse(&entries["index.json"], "index.json").unwrap();
            let path = blob_path(&digest).unwrap();
            let mut manifest = parse_manifest(&entries.remove(&path).unwrap(), &path).unwrap();
            if expected == "Size mismatch" {
                manifest.layers[0].size += 1;
            } else {
                manifest.layers.pop();
            }
            let bytes = canonical_json(&manifest).unwrap();
            let descriptor = describe(MANIFEST, &bytes);
            digest.clone_from(&descriptor.digest);
            index.manifests[0] = descriptor;
            entries.insert(blob_path(&digest).unwrap(), bytes);
            entries.insert("index.json".into(), canonical_json(&index).unwrap());
        }
        let broken = write_tar(&entries).unwrap();
        let read_error = read(broken.as_slice()).unwrap_err();
        let stream_error =
            verify_manifest_digest(std::io::Cursor::new(&broken), &digest).unwrap_err();
        for error in [read_error, stream_error] {
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }
    }
}

#[test]
fn streaming_manifest_check_hashes_the_bytes_not_just_the_index() {
    let (config, layers) = sample();
    let (digest, archive) = pack(&config, &layers).unwrap();
    verify_manifest_digest(std::io::Cursor::new(&archive), &digest).unwrap();

    let mut entries = archive_entries(&archive[..]).unwrap();
    // Keep the index, the blob path, and the length unchanged.
    entries.get_mut(&blob_path(&digest).unwrap()).unwrap()[0] ^= 1;
    let modified = write_tar(&entries).unwrap();
    let error = verify_manifest_digest(std::io::Cursor::new(&modified), &digest).unwrap_err();
    assert!(format!("{error:#}").contains("Digest mismatch"));
}

#[test]
fn refuses_a_layer_whose_bytes_do_not_match_its_digest() {
    let (config, layers) = sample();
    let (digest, archive) = pack(&config, &layers).unwrap();
    let mut entries = archive_entries(&archive[..]).unwrap();
    let path = blob_path(&config.components.values().next().unwrap().wasm).unwrap();
    entries.get_mut(&path).unwrap()[0] ^= 1;
    let broken = write_tar(&entries).unwrap();
    let error = format!("{:#}", read(&broken[..]).unwrap_err());
    assert!(error.contains(&path), "{error}");
    let error = verify_manifest_digest(std::io::Cursor::new(&broken), &digest).unwrap_err();
    assert!(format!("{error:#}").contains(&format!("Digest mismatch for {path}")));
    entries.remove(&path);
    let missing = write_tar(&entries).unwrap();
    let error = verify_manifest_digest(std::io::Cursor::new(&missing), &digest).unwrap_err();
    assert!(format!("{error:#}").contains(&format!("Missing package entry {path}")));
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
fn streaming_verifier_scans_shared_initial_files_once() {
    struct Counted<R>(R, usize);
    impl<R: Read> Read for Counted<R> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let count = self.0.read(bytes)?;
            self.1 += count;
            Ok(count)
        }
    }
    impl<R: Seek> Seek for Counted<R> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(position)
        }
    }
    let (mut config, mut blobs) = sample();
    let nested = write_tar(&BTreeMap::from([("00000000".into(), vec![7; 1024 * 1024])])).unwrap();
    let digest = add_blob(&mut blobs, INITIAL_FILES, nested).digest;
    for (name, component) in &mut config.components {
        component.provision.files = Some(PackageInitialFiles {
            layer: digest.clone(),
            entries: BTreeMap::from([(
                "00000000".into(),
                PackageInitialFile {
                    target: AgentFilePath::from_abs_str(&format!("/{}", name.0)).unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
            )]),
        });
    }
    let (manifest, archive) = pack(&config, &blobs).unwrap();
    let mut input = Counted(std::io::Cursor::new(&archive), 0);
    verify_manifest_digest(&mut input, &manifest).unwrap();
    assert!(
        input.1 < archive.len() * 5 / 2,
        "read {} bytes from a {}-byte archive",
        input.1,
        archive.len()
    );
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
        let (digest, tar) = pack_archive(&config, &blobs).unwrap();
        let error = format!("{:#}", read(tar.as_slice()).unwrap_err());
        assert!(error.contains(expected), "{error}");
        assert!(
            error.contains(&blob_path(&descriptor.digest).unwrap()),
            "{error}"
        );
        let error = verify_manifest_digest(std::io::Cursor::new(&tar), &digest).unwrap_err();
        assert!(format!("{error:#}").contains(expected));
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

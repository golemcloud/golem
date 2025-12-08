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

use crate::model::{
    ApplicationName, ComposableAppGroupName, DocDependency, DocDependencyEnvVar,
    DocDependencyGroup, GuestLanguage, PackageName, SdkOverrides, TargetExistsResolveDecision,
    TargetExistsResolveMode, Template, TemplateKind, TemplateMetadata, TemplateName,
    TemplateParameters, Transform,
};
use anyhow::Context;
use include_dir::{include_dir, Dir, DirEntry};
use indoc::{formatdoc, indoc};
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fs, io};

pub mod model;

#[cfg(test)]
test_r::enable!();

static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");
static WIT: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/wit/deps");

static APP_MANIFEST_HEADER: &str = indoc! {"
# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.3.0/golem.schema.json
# Schema for vscode-yaml:
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.3.0/golem.schema.json

# Field reference: https://learn.golem.cloud/app-manifest#field-reference
# Creating HTTP APIs: https://learn.golem.cloud/invoke/making-custom-apis
"};

static GOLEM_RUST_VERSION: &str = "1.10.0";
static GOLEM_TS_VERSION: &str = "0.0.68";
static GOLEM_AI_VERSION: &str = "v0.4.0-dev.1";
static GOLEM_AI_SUFFIX: &str = "-dev.wasm";

fn all_templates(dev_mode: bool) -> Vec<Template> {
    let mut result: Vec<Template> = vec![];
    for entry in TEMPLATES.entries() {
        if let Some(lang_dir) = entry.as_dir() {
            let lang_dir_name = lang_dir.path().file_name().unwrap().to_str().unwrap();
            if let Some(lang) = GuestLanguage::from_string(lang_dir_name) {
                for sub_entry in lang_dir.entries() {
                    if let Some(template_dir) = sub_entry.as_dir() {
                        let template_dir_name =
                            template_dir.path().file_name().unwrap().to_str().unwrap();
                        if template_dir_name != "INSTRUCTIONS"
                            && !template_dir_name.starts_with('.')
                        {
                            let template = parse_template(
                                lang,
                                lang_dir.path(),
                                Path::new("INSTRUCTIONS"),
                                template_dir.path(),
                            );

                            if dev_mode || !template.dev_only {
                                result.push(template);
                            }
                        }
                    }
                }
            } else {
                panic!("Invalid guest language name: {lang_dir_name}");
            }
        }
    }
    result
}

pub fn all_standalone_templates() -> Vec<Template> {
    all_templates(true)
        .into_iter()
        .filter(|template| matches!(template.kind, TemplateKind::Standalone))
        .collect()
}

#[derive(Debug, Default)]
pub struct ComposableAppTemplate {
    pub common: Option<Template>,
    pub components: BTreeMap<TemplateName, Template>,
}

pub fn all_composable_app_templates(
    dev_mode: bool,
) -> BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>> {
    let mut templates =
        BTreeMap::<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>>::new();

    fn app_templates<'a>(
        templates: &'a mut BTreeMap<
            GuestLanguage,
            BTreeMap<ComposableAppGroupName, ComposableAppTemplate>,
        >,
        language: GuestLanguage,
        group: &ComposableAppGroupName,
    ) -> &'a mut ComposableAppTemplate {
        let groups = templates.entry(language).or_default();
        if !groups.contains_key(group) {
            groups.insert(group.clone(), ComposableAppTemplate::default());
        }
        groups.get_mut(group).unwrap()
    }

    for template in all_templates(dev_mode) {
        match &template.kind {
            TemplateKind::Standalone => continue,
            TemplateKind::ComposableAppCommon { group, .. } => {
                let common = &mut app_templates(&mut templates, template.language, group).common;
                if let Some(common) = common {
                    panic!(
                        "Multiple common templates were found for {} - {}, template paths: {}, {}",
                        template.language,
                        group,
                        common.template_path.display(),
                        template.template_path.display()
                    );
                }
                *common = Some(template);
            }
            TemplateKind::ComposableAppComponent { group } => {
                app_templates(&mut templates, template.language, group)
                    .components
                    .insert(template.name.clone(), template);
            }
        }
    }

    templates
}

pub fn instantiate_template(
    template: &Template,
    parameters: &TemplateParameters,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<String> {
    instantiate_directory(
        &TEMPLATES,
        &template.template_path,
        &parameters.target_path,
        template,
        parameters,
        resolve_mode,
    )?;
    let wit_deps_targets = {
        match &template.wit_deps_targets {
            Some(paths) => paths
                .iter()
                .map(|path| parameters.target_path.join(path))
                .collect(),
            None => vec![parameters.target_path.join("wit").join("deps")],
        }
    };
    for wit_dep in &template.wit_deps {
        for target_wit_deps in &wit_deps_targets {
            let name = wit_dep.file_name().unwrap().to_str().unwrap();
            let target = target_wit_deps.join(name);
            copy_all(&WIT, wit_dep, &target, TargetExistsResolveMode::MergeOrSkip)?;
        }
    }
    Ok(render_template_instructions(template, parameters))
}

pub fn add_component_by_template(
    common_template: Option<&Template>,
    component_template: Option<&Template>,
    target_path: &Path,
    application_name: &ApplicationName,
    package_name: &PackageName,
    sdk_overrides: Option<&SdkOverrides>,
) -> anyhow::Result<()> {
    let parameters = TemplateParameters {
        application_name: application_name.clone(),
        component_name: package_name.to_string_with_colon().into(),
        package_name: package_name.clone(),
        target_path: target_path.into(),
        sdk_overrides: sdk_overrides.cloned().unwrap_or_default(),
    };

    if let Some(common_template) = common_template {
        let skip = {
            if let TemplateKind::ComposableAppCommon {
                skip_if_exists: Some(file),
                ..
            } = &common_template.kind
            {
                target_path.join(file).exists()
            } else {
                false
            }
        };

        if !skip {
            instantiate_template(
                common_template,
                &parameters,
                TargetExistsResolveMode::MergeOrSkip,
            )
            .context(format!(
                "Instantiating common template {}",
                common_template.name
            ))?;
        }
    }

    if let Some(component_template) = component_template {
        instantiate_template(
            component_template,
            &parameters,
            TargetExistsResolveMode::MergeOrFail,
        )
        .context(format!(
            "Instantiating component template {}",
            component_template.name
        ))?;
    }

    Ok(())
}

pub fn render_template_instructions(
    template: &Template,
    parameters: &TemplateParameters,
) -> String {
    transform(
        &template.instructions,
        parameters,
        &[Transform::PackageAndComponent],
    )
}

fn instantiate_directory(
    catalog: &Dir<'_>,
    source: &Path,
    target: &Path,
    template: &Template,
    parameters: &TemplateParameters,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in catalog
        .get_dir(source)
        .unwrap_or_else(|| panic!("Could not find entry {source:?}"))
        .entries()
    {
        let name = entry.path().file_name().unwrap().to_str().unwrap();
        if name != "metadata.json" {
            let name = file_name_transform(name, parameters);
            match entry {
                DirEntry::Dir(dir) => {
                    instantiate_directory(
                        catalog,
                        dir.path(),
                        &target.join(&name),
                        template,
                        parameters,
                        resolve_mode,
                    )?;
                }
                DirEntry::File(file) => {
                    let content_transform = match (template.kind.is_common(), name.as_str()) {
                        (true, "golem.yaml") => {
                            vec![Transform::ManifestHints, Transform::ApplicationName]
                        }
                        (true, "package.json") => vec![Transform::TsSdk],
                        (true, "Cargo.toml") => vec![Transform::RustSdk],
                        (true, _) => vec![],
                        (false, "golem.yaml") => {
                            vec![
                                Transform::ManifestHints,
                                Transform::PackageAndComponent,
                                Transform::ApplicationName,
                            ]
                        }
                        (false, _) => vec![Transform::PackageAndComponent],
                    };

                    instantiate_file(
                        catalog,
                        file.path(),
                        &target.join(&name),
                        parameters,
                        content_transform,
                        resolve_mode,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn instantiate_file(
    catalog: &Dir<'_>,
    source: &Path,
    target: &Path,
    parameters: &TemplateParameters,
    content_transforms: Vec<Transform>,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    match get_resolved_contents(catalog, source, target, resolve_mode)? {
        Some(contents) => {
            if content_transforms.is_empty() {
                fs::write(target, contents)
            } else {
                fs::write(
                    target,
                    transform(
                        std::str::from_utf8(contents.as_ref()).map_err(|err| {
                            io::Error::other(format!(
                                "Failed to decode as utf8, source: {}, err: {}",
                                source.display(),
                                err
                            ))
                        })?,
                        parameters,
                        &content_transforms,
                    ),
                )
            }
        }
        None => Ok(()),
    }
}

fn copy(
    catalog: &Dir<'_>,
    source: &Path,
    target: &Path,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    match get_resolved_contents(catalog, source, target, resolve_mode)? {
        Some(contents) => fs::write(target, contents),
        None => Ok(()),
    }
}

fn copy_all(
    catalog: &Dir<'_>,
    source_path: &Path,
    target_path: &Path,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    let source_dir = catalog.get_dir(source_path).ok_or_else(|| {
        io::Error::other(format!(
            "Could not find dir {} in catalog",
            source_path.display()
        ))
    })?;

    fs::create_dir_all(target_path)?;

    for file in source_dir.files() {
        copy(
            catalog,
            file.path(),
            &target_path.join(file.path().file_name().unwrap().to_str().unwrap()),
            resolve_mode,
        )?;
    }

    Ok(())
}

fn transform(
    str: impl AsRef<str>,
    parameters: &TemplateParameters,
    transforms: &[Transform],
) -> String {
    let transform_pack_and_comp = |str: &str| -> String {
        str.replace(
            "componentnameapi",
            &format!("{}api", parameters.component_name.parts().join("")),
        )
        .replace("componentname", parameters.component_name.as_str())
        .replace("component-name", &parameters.component_name.to_kebab_case())
        .replace("ComponentName", &parameters.component_name.to_pascal_case())
        .replace("componentName", &parameters.component_name.to_camel_case())
        .replace("component_name", &parameters.component_name.to_snake_case())
        .replace(
            "pack::name",
            &parameters.package_name.to_string_with_double_colon(),
        )
        .replace("pa_ck::na_me", &parameters.package_name.to_rust_binding())
        .replace("pack:name", &parameters.package_name.to_string_with_colon())
        .replace("pack_name", &parameters.package_name.to_snake_case())
        .replace("pack-name", &parameters.package_name.to_kebab_case())
        .replace("pack/name", &parameters.package_name.to_string_with_slash())
        .replace("PackName", &parameters.package_name.to_pascal_case())
        .replace("pack-ns", &parameters.package_name.namespace())
        .replace("PackNs", &parameters.package_name.namespace_title_case())
        .replace("__pack__", &parameters.package_name.namespace_snake_case())
        .replace("__name__", &parameters.package_name.name_snake_case())
        .replace("__cn__", "componentName")
    };

    let transform_manifest_hints = |str: &str| -> String {
        str.replace("# golem-app-manifest-header\n", APP_MANIFEST_HEADER)
            .replace("    # golem-app-manifest-env-doc",
                concat!(
                    "    # Component environment variables can reference system environment variables with minijinja syntax:\n",
                    "    #\n",
                    "    #   env:\n",
                    "    #     ENV_VAR_1: \"{{ ENV_VAR_1 }}\"\n",
                    "    #     RENAMED_VAR_2: \"{{ ENV_VAR_2 }}\"\n",
                    "    #     COMPOSED_VAR_3: \"{{ ENV_VAR_3 }}-{{ ENV_VAR_4}}\"\n",
                    "    #",
                )
            )
            .replace("    # golem-app-manifest-dep-env-vars-doc", dep_env_vars_doc())
            .replace("    # golem-app-manifest-deps-doc", deps_doc())
            .replace("    # golem-app-manifest-env-presets",
                     "    # TODO: atomic\n"
            )
    };

    let transform_app_name =
        |str: &str| -> String { str.replace("app-name", parameters.application_name.as_str()) };

    let transform_rust_sdk = |str: &str| -> String {
        let path_or_version = {
            if let Some(rust_path) = &parameters.sdk_overrides.rust_path {
                format!(r#"path = "{}""#, rust_path)
            } else {
                format!(
                    r#"version = "{}""#,
                    parameters
                        .sdk_overrides
                        .rust_version
                        .as_deref()
                        .unwrap_or(GOLEM_RUST_VERSION)
                )
            }
        };

        str.replace("GOLEM_RUST_VERSION_OR_PATH", &path_or_version)
    };

    let transform_ts_sdk = |str: &str| -> String {
        let (sdk_version_or_path, typegen_version_or_path) = {
            if let Some(ts_packages_path) = parameters.sdk_overrides.ts_packages_path.as_ref() {
                (
                    format!("{}/golem-ts-sdk", ts_packages_path),
                    format!("{}/golem-ts-typegen", ts_packages_path),
                )
            } else {
                let version = parameters
                    .sdk_overrides
                    .ts_version
                    .as_deref()
                    .unwrap_or(GOLEM_TS_VERSION);
                (version.to_string(), version.to_string())
            }
        };

        str.replace("GOLEM_TS_SDK_VERSION_OR_PATH", &sdk_version_or_path)
            .replace("GOLEM_TS_TYPEGEN_VERSION_OR_PATH", &typegen_version_or_path)
    };

    let mut transformed = str.as_ref().to_string();

    for transform in transforms {
        transformed = match transform {
            Transform::PackageAndComponent => transform_pack_and_comp(&transformed),
            Transform::ManifestHints => transform_manifest_hints(&transformed),
            Transform::TsSdk => transform_ts_sdk(&transformed),
            Transform::RustSdk => transform_rust_sdk(&transformed),
            Transform::ApplicationName => transform_app_name(&transformed),
        };
    }

    transformed
}

fn file_name_transform(str: impl AsRef<str>, parameters: &TemplateParameters) -> String {
    transform(str, parameters, &[Transform::PackageAndComponent])
        .replace("Cargo.toml._", "Cargo.toml")
    // HACK because cargo package ignores every subdirectory containing a Cargo.toml
}

fn check_target(
    target: &Path,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<Option<TargetExistsResolveDecision>> {
    if !target.exists() {
        return Ok(None);
    }

    let get_merge = || -> io::Result<Option<TargetExistsResolveDecision>> {
        let file_name = target
            .file_name()
            .ok_or_else(|| {
                io::Error::other(format!(
                    "Failed to get file name for target: {}",
                    target.display()
                ))
            })
            .and_then(|file_name| {
                file_name.to_str().ok_or_else(|| {
                    io::Error::other(format!(
                        "Failed to convert file name to string: {}",
                        file_name.to_string_lossy()
                    ))
                })
            })?;

        match file_name {
            ".gitignore" => {
                let target = target.to_path_buf();
                let current_content = fs::read_to_string(&target)?;
                Ok(Some(TargetExistsResolveDecision::Merge(Box::new(
                    move |new_content: &[u8]| -> io::Result<Vec<u8>> {
                        Ok(current_content
                            .lines()
                            .chain(
                                std::str::from_utf8(new_content).map_err(|err| {
                                    io::Error::other(format!(
                                        "Failed to decode new content for merge as utf8, target: {}, err: {}",
                                        target.display(),
                                        err
                                    ))
                                })?.lines(),
                            )
                            .collect::<BTreeSet<&str>>()
                            .iter()
                            .join("\n")
                            .into_bytes())
                    },
                ))))
            }
            _ => Ok(None),
        }
    };

    let target_already_exists = || {
        Err(io::Error::other(format!(
            "Target ({}) already exists!",
            target.display()
        )))
    };

    match resolve_mode {
        TargetExistsResolveMode::Skip => Ok(Some(TargetExistsResolveDecision::Skip)),
        TargetExistsResolveMode::MergeOrSkip => match get_merge()? {
            Some(merge) => Ok(Some(merge)),
            None => Ok(Some(TargetExistsResolveDecision::Skip)),
        },
        TargetExistsResolveMode::Fail => target_already_exists(),
        TargetExistsResolveMode::MergeOrFail => match get_merge()? {
            Some(merge) => Ok(Some(merge)),
            None => target_already_exists(),
        },
    }
}

fn get_contents<'a>(catalog: &Dir<'a>, source: &'a Path) -> io::Result<&'a [u8]> {
    Ok(catalog
        .get_file(source)
        .ok_or_else(|| io::Error::other(format!("Could not find entry {}", source.display())))?
        .contents())
}

fn get_resolved_contents<'a>(
    catalog: &Dir<'a>,
    source: &'a Path,
    target: &'a Path,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<Option<Cow<'a, [u8]>>> {
    match check_target(target, resolve_mode)? {
        None => Ok(Some(Cow::Borrowed(get_contents(catalog, source)?))),
        Some(TargetExistsResolveDecision::Skip) => Ok(None),
        Some(TargetExistsResolveDecision::Merge(merge)) => {
            Ok(Some(Cow::Owned(merge(get_contents(catalog, source)?)?)))
        }
    }
}

fn parse_template(
    lang: GuestLanguage,
    lang_path: &Path,
    default_instructions_file_name: &Path,
    template_root: &Path,
) -> Template {
    let raw_metadata = TEMPLATES
        .get_file(template_root.join("metadata.json"))
        .expect("Failed to read metadata JSON")
        .contents();
    let metadata = serde_json::from_slice::<TemplateMetadata>(raw_metadata)
        .expect("Failed to parse metadata JSON");

    let kind = match (metadata.app_common_group, metadata.app_component_group) {
        (None, None) => TemplateKind::Standalone,
        (Some(group), None) => TemplateKind::ComposableAppCommon {
            group: group.into(),
            skip_if_exists: metadata.app_common_skip_if_exists.map(PathBuf::from),
        },
        (None, Some(group)) => TemplateKind::ComposableAppComponent {
            group: group.into(),
        },
        (Some(_), Some(_)) => panic!(
            "Only one of appCommonGroup and appComponentGroup can be specified, template root: {}",
            template_root.display()
        ),
    };

    let instructions = match &kind {
        TemplateKind::Standalone => {
            let instructions_path = match metadata.instructions {
                Some(instructions_file_name) => lang_path.join(instructions_file_name),
                None => lang_path.join(default_instructions_file_name),
            };

            let raw_instructions = TEMPLATES
                .get_file(instructions_path)
                .expect("Failed to read instructions")
                .contents();

            String::from_utf8(raw_instructions.to_vec()).expect("Failed to decode instructions")
        }
        TemplateKind::ComposableAppCommon { .. } => "".to_string(),
        TemplateKind::ComposableAppComponent { .. } => "".to_string(),
    };

    let name: TemplateName = {
        let name = template_root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // TODO: this is just a quickfix for hiding "<lang>-app-<component>" prefixes, let's decide later if we want
        //       reorganize the template directories directly
        let segments = name.split("-").collect::<Vec<_>>();
        match segments.iter().position(|&s| s == "app") {
            Some(idx) => match segments.get(idx + 1) {
                Some(&"component") => segments[idx + 2..].join("-").into(),
                _ => name.into(),
            },
            None => name.into(),
        }
    };

    let mut wit_deps: Vec<PathBuf> = vec![];
    if metadata.requires_golem_host_wit.unwrap_or(false) {
        WIT.dirs()
            .filter(|&dir| dir.path().starts_with("golem"))
            .map(|dir| dir.path())
            .for_each(|path| {
                wit_deps.push(path.to_path_buf());
            });

        wit_deps.push(PathBuf::from("golem-1.x"));
        wit_deps.push(PathBuf::from("golem-rpc"));
        wit_deps.push(PathBuf::from("golem-rdbms"));
        wit_deps.push(PathBuf::from("golem-agent"));
        wit_deps.push(PathBuf::from("golem-durability"));
    }
    if metadata.requires_wasi.unwrap_or(false) {
        wit_deps.push(PathBuf::from("blobstore"));
        wit_deps.push(PathBuf::from("cli"));
        wit_deps.push(PathBuf::from("clocks"));
        wit_deps.push(PathBuf::from("config"));
        wit_deps.push(PathBuf::from("filesystem"));
        wit_deps.push(PathBuf::from("http"));
        wit_deps.push(PathBuf::from("io"));
        wit_deps.push(PathBuf::from("keyvalue"));
        wit_deps.push(PathBuf::from("logging"));
        wit_deps.push(PathBuf::from("random"));
        wit_deps.push(PathBuf::from("sockets"));
    }

    Template {
        name,
        kind,
        language: lang,
        description: metadata.description,
        template_path: template_root.to_path_buf(),
        instructions,
        wit_deps,
        wit_deps_targets: metadata
            .wit_deps_paths
            .map(|dirs| dirs.iter().map(PathBuf::from).collect()),
        dev_only: metadata.dev_only.unwrap_or(false),
    }
}

static DOC_DEPENDENCIES: OnceLock<Vec<DocDependencyGroup>> = OnceLock::new();

fn doc_dependencies() -> &'static Vec<DocDependencyGroup> {
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

    fn init() -> Vec<DocDependencyGroup> {
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
    }

    DOC_DEPENDENCIES.get_or_init(init)
}

static DEP_ENV_VARS_DOC: OnceLock<String> = OnceLock::new();

fn dep_env_vars_doc() -> &'static str {
    fn init(groups: &[DocDependencyGroup], indent: &str) -> String {
        let mut out = String::new();

        for group in groups {
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
    }

    DEP_ENV_VARS_DOC.get_or_init(move || init(doc_dependencies(), "    "))
}

static DEPS_DOC: OnceLock<String> = OnceLock::new();

fn deps_doc() -> &'static str {
    fn init(groups: &[DocDependencyGroup], indent: &str) -> String {
        let mut out = String::new();

        out.push_str(&formatdoc! {"
        {indent}# The following block contains commented-out dependencies for various Golem AI libraries.
        {indent}# For each area (such as LLM, Search, etc) only one of the providers can be commented out.
        {indent}# If no provider dependency is commented out, then using that AI API will be a runtime failure.
        ",
        });

        for group in groups {
            out.push_str(&doc_group_header(indent, group));

            for dep in &group.dependencies {
                if dep.url.is_empty() {
                    continue;
                }

                out.push_str(&doc_dep_header(indent, dep));
                out.push_str(&format!("{indent}# - type: wasm\n", indent = indent));
                out.push_str(&format!("{indent}#   url: {}\n", dep.url, indent = indent));
                out.push('\n');
            }

            out.push('\n');
        }

        out.trim_end().to_string()
    }

    DEPS_DOC.get_or_init(move || init(doc_dependencies(), "    "))
}

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

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

use crate::app::template::repo::TEMPLATES_DIR;
use crate::app::template::snippet::{APP_MANIFEST_HEADER, DEP_ENV_VARS_DOC};
use crate::app::template::AppTemplate;
use crate::{fs, SdkOverrides};
use anyhow::{anyhow, bail};
use golem_common::base_model::application::ApplicationName;
use golem_common::base_model::component::ComponentName;
use heck::{ToKebabCase, ToSnakeCase};
use include_dir::{Dir, DirEntry};
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const ON_DEMAND_COMMON_HASH_FILE_NAME: &str = ".golem-template-content-hash";

#[derive(Debug, Copy, Clone)]
enum TargetExistsResolveMode {
    #[allow(dead_code)]
    Skip,
    MergeOrSkip,
    Fail,
    MergeOrFail,
}

type MergeContents = Box<dyn FnOnce(&[u8]) -> anyhow::Result<Vec<u8>>>;

enum TargetExistsResolveDecision {
    Skip,
    Merge(MergeContents),
}

pub trait TemplateGeneratorTargetFs {
    type Output;

    fn ensure_dir(&self, path: &Path) -> anyhow::Result<()>;
    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()>;
    fn finish(self) -> Self::Output;
    fn is_in_memory(&self) -> bool;
}

#[derive(Debug, Default)]
pub struct StdFs;

impl TemplateGeneratorTargetFs for StdFs {
    type Output = ();

    fn ensure_dir(&self, path: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(path).map(|_| ())
    }

    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()> {
        fs::write_str(path, contents)
    }

    fn finish(self) -> Self::Output {}

    fn is_in_memory(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct InMemoryFs {
    files: BTreeMap<PathBuf, String>,
}

impl InMemoryFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn files(&self) -> &BTreeMap<PathBuf, String> {
        &self.files
    }

    pub fn get(&self, path: &Path) -> Option<&str> {
        self.files.get(path).map(|s| s.as_str())
    }
}

impl TemplateGeneratorTargetFs for InMemoryFs {
    type Output = InMemoryFs;

    fn ensure_dir(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()> {
        self.files.insert(path.to_path_buf(), contents);
        Ok(())
    }

    fn finish(self) -> Self::Output {
        self
    }

    fn is_in_memory(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Transform {
    ComponentName,
    ManifestHints,
    TsSdk,
    RustSdk,
    ApplicationName,
}

#[derive(Debug, Clone)]
struct GeneratorContext<'a> {
    template: &'a AppTemplate,
    application_name: Option<&'a ApplicationName>,
    component_name: Option<&'a ComponentName>,
    target_path: &'a Path,
    sdk_overrides: &'a SdkOverrides,
    resolve_mode: TargetExistsResolveMode,
}

pub fn generate_commons_by_template<T: TemplateGeneratorTargetFs>(
    template: &AppTemplate,
    application_name: &ApplicationName,
    target_path: &Path,
    sdk_overrides: &SdkOverrides,
    mut target: T,
) -> anyhow::Result<T::Output> {
    if !template.metadata.is_common() {
        bail!("Template {} is not a common template", template.name);
    }

    generate_root_directory(
        &mut target,
        &GeneratorContext {
            template,
            application_name: Some(application_name),
            component_name: None,
            target_path,
            sdk_overrides,
            resolve_mode: TargetExistsResolveMode::MergeOrSkip,
        },
    )?;
    Ok(target.finish())
}

pub fn generate_on_demand_commons_by_template<T: TemplateGeneratorTargetFs>(
    template: &AppTemplate,
    target_path: &Path,
    sdk_overrides: &SdkOverrides,
    mut target: T,
) -> anyhow::Result<T::Output> {
    if !template.metadata.is_common_on_demand() {
        bail!(
            "Template {} is not a common on demand template",
            template.name
        );
    }

    if let Some(content_hash) = template.content_hash.as_deref() {
        let hash_path = target_path.join(ON_DEMAND_COMMON_HASH_FILE_NAME);
        if target_path.exists() && hash_path.exists() {
            let stored_hash = fs::read_to_string(&hash_path)?;
            if stored_hash.trim() == content_hash {
                return Ok(target.finish());
            }
        }
    }

    fs::remove(target_path)?;

    generate_root_directory(
        &mut target,
        &GeneratorContext {
            template,
            application_name: None,
            component_name: None,
            target_path,
            sdk_overrides,
            resolve_mode: TargetExistsResolveMode::Fail,
        },
    )?;

    if let Some(content_hash) = template.content_hash.as_deref() {
        let hash_path = target_path.join(ON_DEMAND_COMMON_HASH_FILE_NAME);
        fs::write_str(hash_path, content_hash)?;
    }

    Ok(target.finish())
}

pub fn generate_component_by_template<T: TemplateGeneratorTargetFs>(
    template: &AppTemplate,
    target_path: &Path,
    application_name: &ApplicationName,
    component_name: &ComponentName,
    sdk_overrides: &SdkOverrides,
    mut target: T,
) -> anyhow::Result<T::Output> {
    if !template.metadata.is_component() {
        bail!("Template {} is not a component template", template.name);
    }

    generate_root_directory(
        &mut target,
        &GeneratorContext {
            template,
            application_name: Some(application_name),
            component_name: Some(component_name),
            target_path,
            sdk_overrides,
            resolve_mode: TargetExistsResolveMode::MergeOrFail,
        },
    )?;
    Ok(target.finish())
}

fn generate_root_directory<T: TemplateGeneratorTargetFs>(
    target: &mut T,
    ctx: &GeneratorContext<'_>,
) -> anyhow::Result<()> {
    generate_directory(
        target,
        ctx,
        &TEMPLATES_DIR,
        &ctx.template.template_path,
        ctx.target_path,
    )
}

fn generate_directory<T: TemplateGeneratorTargetFs>(
    target: &mut T,
    ctx: &GeneratorContext<'_>,
    templates_dir: &Dir<'_>,
    source: &Path,
    target_path: &Path,
) -> anyhow::Result<()> {
    target.ensure_dir(target_path)?;
    for entry in templates_dir
        .get_dir(source)
        .unwrap_or_else(|| panic!("Could not find entry {source:?}"))
        .entries()
    {
        let entry_path = entry.path();
        let name = entry_path.file_name().unwrap().to_str().unwrap();
        if name != "metadata.json" {
            let name = transform_file_name(ctx, name);
            match entry {
                DirEntry::Dir(dir) => {
                    generate_directory(
                        target,
                        ctx,
                        templates_dir,
                        dir.path(),
                        &target_path.join(&name),
                    )?;
                }
                DirEntry::File(file) => {
                    let content_transform = match (ctx.template.metadata.is_common(), name.as_str())
                    {
                        (true, "golem.yaml") => {
                            vec![Transform::ManifestHints, Transform::ApplicationName]
                        }
                        (true, "package.json") => vec![Transform::TsSdk],
                        (true, "Cargo.toml") => vec![Transform::RustSdk],
                        (true, _) => vec![],
                        (false, "golem.yaml") => {
                            vec![
                                Transform::ManifestHints,
                                Transform::ComponentName,
                                Transform::ApplicationName,
                            ]
                        }
                        (false, _) => vec![Transform::ComponentName],
                    };

                    instantiate_file(
                        target,
                        ctx,
                        templates_dir,
                        file.path(),
                        &target_path.join(&name),
                        content_transform,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn instantiate_file<T: TemplateGeneratorTargetFs>(
    target: &mut T,
    ctx: &GeneratorContext<'_>,
    dir: &Dir<'_>,
    source: &Path,
    target_path: &Path,
    content_transforms: Vec<Transform>,
) -> anyhow::Result<()> {
    match get_resolved_contents(target, ctx, dir, source, target_path)? {
        Some(contents) => {
            let contents = std::str::from_utf8(contents.as_ref()).map_err(|err| {
                anyhow!(
                    "Failed to decode as utf8, source: {}, err: {}",
                    source.display(),
                    err
                )
            })?;

            let rendered = if content_transforms.is_empty() {
                contents.to_string()
            } else {
                transform(ctx, contents, &content_transforms)
            };

            target.write_file(target_path, rendered)
        }
        None => Err(anyhow!(
            "Failed to resolve template contents for {}",
            source.display()
        )),
    }
}

fn transform(ctx: &GeneratorContext<'_>, str: impl AsRef<str>, transforms: &[Transform]) -> String {
    let transform_pack_and_comp = |str: &str| -> String {
        match &ctx.component_name {
            Some(component_name) => str
                .replace("componentname", component_name.as_str())
                .replace("component-name", &component_name.0.to_kebab_case())
                .replace("component_name", &component_name.0.to_snake_case())
                .replace("__cn__", "componentName"),
            None => str.to_string(),
        }
    };

    let transform_manifest_hints = |str: &str| -> String {
        str.replace("# golem-app-manifest-header\n", &APP_MANIFEST_HEADER)
            .replace("    # golem-app-manifest-env-doc",
                     concat!(
                     "    # Component environment variables can reference system environment variables with minijinja syntax:\n",
                     "    #\n",
                     "    #   env:\n",
                     "    #     ENV_VAR_1: \"{{ ENV_VAR_1 }}\"\n",
                     "    #     RENAMED_VAR_2: \"{{ ENV_VAR_2 }}\"\n",
                     "    #     COMPOSED_VAR_3: \"{{ ENV_VAR_3 }}-{{ ENV_VAR_4}}\"\n",
                     "    #",
                     ),
            )
            .replace("    # golem-app-manifest-dep-env-vars-doc", &DEP_ENV_VARS_DOC)
    };

    let transform_app_name = |str: &str| -> String {
        match &ctx.application_name {
            Some(name) => str.replace("app-name", &name.0),
            None => str.to_string(),
        }
    };

    let transform_rust_sdk = |str: &str| -> String {
        str.replace(
            "GOLEM_RUST_VERSION_OR_PATH",
            &ctx.sdk_overrides.golem_rust_dep(),
        )
    };

    let transform_ts_sdk = |str: &str| -> String {
        str.replace(
            "GOLEM_TS_SDK_VERSION_OR_PATH",
            &ctx.sdk_overrides.ts_package_dep("golem-ts-sdk"),
        )
        .replace(
            "GOLEM_TS_TYPEGEN_VERSION_OR_PATH",
            &ctx.sdk_overrides.ts_package_dep("golem-ts-typegen"),
        )
    };

    let mut transformed = str.as_ref().to_string();

    for transform in transforms {
        transformed = match transform {
            Transform::ComponentName => transform_pack_and_comp(&transformed),
            Transform::ManifestHints => transform_manifest_hints(&transformed),
            Transform::TsSdk => transform_ts_sdk(&transformed),
            Transform::RustSdk => transform_rust_sdk(&transformed),
            Transform::ApplicationName => transform_app_name(&transformed),
        };
    }

    transformed
}

fn transform_file_name(ctx: &GeneratorContext<'_>, file_name: impl AsRef<str>) -> String {
    transform(ctx, file_name, &[Transform::ComponentName]).replace("Cargo.toml._", "Cargo.toml")
}

fn check_target<T: TemplateGeneratorTargetFs>(
    target_kind: &T,
    target: &Path,
    resolve_mode: TargetExistsResolveMode,
) -> anyhow::Result<Option<TargetExistsResolveDecision>> {
    if target_kind.is_in_memory() {
        return Ok(None);
    }

    if !target.exists() {
        return Ok(None);
    }

    let get_merge = || -> anyhow::Result<Option<TargetExistsResolveDecision>> {
        let file_name = target
            .file_name()
            .ok_or_else(|| anyhow!("Failed to get file name for target: {}", target.display()))
            .and_then(|file_name| {
                file_name.to_str().ok_or_else(|| {
                    anyhow!(
                        "Failed to convert file name to string: {}",
                        file_name.to_string_lossy()
                    )
                })
            })?;

        match file_name {
            ".gitignore" => {
                let target = target.to_path_buf();
                let current_content = fs::read_to_string(&target)?;
                Ok(Some(TargetExistsResolveDecision::Merge(Box::new(
                    move |new_content: &[u8]| -> anyhow::Result<Vec<u8>> {
                        Ok(current_content
                            .lines()
                            .chain(
                                std::str::from_utf8(new_content).map_err(|err| {
                                    anyhow!(
                                        "Failed to decode new content for merge as utf8, target: {}, err: {}",
                                        target.display(),
                                        err
                                    )
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
        Err(anyhow!(format!(
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

fn get_contents<'a>(dir: &Dir<'a>, source: &'a Path) -> anyhow::Result<&'a [u8]> {
    Ok(dir
        .get_file(source)
        .ok_or_else(|| anyhow!("Could not find entry {}", source.display()))?
        .contents())
}

fn get_resolved_contents<'a, T: TemplateGeneratorTargetFs>(
    target_kind: &T,
    ctx: &GeneratorContext<'a>,
    dir: &Dir<'a>,
    source: &'a Path,
    target: &'a Path,
) -> anyhow::Result<Option<Cow<'a, [u8]>>> {
    match check_target(target_kind, target, ctx.resolve_mode)? {
        None => Ok(Some(Cow::Borrowed(get_contents(dir, source)?))),
        Some(TargetExistsResolveDecision::Skip) => Ok(None),
        Some(TargetExistsResolveDecision::Merge(merge)) => {
            Ok(Some(Cow::Owned(merge(get_contents(dir, source)?)?)))
        }
    }
}

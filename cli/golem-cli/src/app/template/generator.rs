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

use crate::app::template::repo::TEMPLATES_DIR;
use crate::app::template::snippet::{APP_MANIFEST_HEADER, DEP_ENV_VARS_DOC};
use crate::app::template::AppTemplate;
use crate::{fs, SdkOverrides};
use anyhow::{anyhow, bail};
use golem_common::base_model::application::ApplicationName;
use golem_common::base_model::component::ComponentName;
use heck::{ToKebabCase, ToSnakeCase};
use include_dir::{Dir, DirEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ON_DEMAND_COMMON_HASH_FILE_NAME: &str = ".golem-template-content-hash";

pub trait TemplateGeneratorTargetFs {
    type Output;

    fn exists(&self, path: &Path) -> bool;
    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()>;
    fn finish(self) -> Self::Output;
}

#[derive(Debug, Default)]
pub struct StdFs;

impl TemplateGeneratorTargetFs for StdFs {
    type Output = ();

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()> {
        fs::write_str(path, contents)
    }

    fn finish(self) -> Self::Output {}
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

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn write_file(&mut self, path: &Path, contents: String) -> anyhow::Result<()> {
        self.files.insert(path.to_path_buf(), contents);
        Ok(())
    }

    fn finish(self) -> Self::Output {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Transform {
    ComponentDir,
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
    application_dir: &'a Path,
    component_name: Option<&'a ComponentName>,
    component_dir: Option<&'a Path>,
    target_path: &'a Path,
    sdk_overrides: &'a SdkOverrides,
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
            application_dir: target_path,
            component_name: None,
            component_dir: None,
            target_path,
            sdk_overrides,
        },
    )?;
    Ok(target.finish())
}

pub fn generate_on_demand_commons_by_template<T: TemplateGeneratorTargetFs>(
    template: &AppTemplate,
    application_dir: &Path,
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
            application_dir,
            component_name: None,
            component_dir: None,
            target_path,
            sdk_overrides,
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
    application_name: &ApplicationName,
    application_dir: &Path,
    component_name: &ComponentName,
    component_dir: &Path,
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
            application_dir,
            component_dir: Some(component_dir),
            target_path: application_dir,
            sdk_overrides,
        },
    )?;
    Ok(target.finish())
}

pub fn generate_agent_by_template<T: TemplateGeneratorTargetFs>(
    template: &AppTemplate,
    application_name: &ApplicationName,
    application_dir: &Path,
    component_name: &ComponentName,
    component_dir: &Path,
    sdk_overrides: &SdkOverrides,
    mut target: T,
) -> anyhow::Result<T::Output> {
    if !template.metadata.is_agent() {
        bail!("Template {} is not an agent template", template.name);
    }

    generate_root_directory(
        &mut target,
        &GeneratorContext {
            template,
            application_name: Some(application_name),
            application_dir,
            component_name: Some(component_name),
            component_dir: Some(component_dir),
            target_path: application_dir,
            sdk_overrides,
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
    for entry in templates_dir
        .get_dir(source)
        .unwrap_or_else(|| panic!("Could not find entry {source:?}"))
        .entries()
    {
        let entry_path = entry.path();
        let name = fs::file_name_to_str(entry_path)?;

        if name == "metadata.json" {
            continue;
        }

        let name = transform_file_name(ctx, name)?;
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
                let content_transform = match (
                    ctx.template.metadata.is_common()
                        || ctx.template.metadata.is_common_on_demand(),
                    name.as_str(),
                ) {
                    (true, "golem.yaml") => {
                        vec![Transform::ManifestHints, Transform::ApplicationName]
                    }
                    (true, "package.json") => vec![Transform::TsSdk],
                    (true, "Cargo.toml") => vec![Transform::RustSdk],
                    (true, _) => vec![],
                    (false, "golem.yaml") => {
                        vec![
                            Transform::ManifestHints,
                            Transform::ComponentDir,
                            Transform::ComponentName,
                            Transform::ApplicationName,
                        ]
                    }
                    (false, "Cargo.toml") => vec![Transform::ComponentName, Transform::RustSdk],
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
    let target_path = transform_target_file_path(ctx, target_path)?;

    if target.exists(&target_path) {
        bail!("Target {} already exists", target_path.display());
    }

    let contents = get_contents(dir, source)?;
    let rendered = if content_transforms.is_empty() {
        contents.to_string()
    } else {
        transform(ctx, contents, &content_transforms)?
    };

    target.write_file(&target_path, rendered)
}

fn transform(
    ctx: &GeneratorContext<'_>,
    str: impl AsRef<str>,
    transforms: &[Transform],
) -> anyhow::Result<String> {
    let transform_component = |str: &str| -> String {
        match &ctx.component_name {
            Some(component_name) => str
                .replace("componentname", component_name.as_str())
                .replace("component-name", &component_name.0.to_kebab_case())
                .replace("component_name", &component_name.0.to_snake_case())
                .replace("__cn__", "componentName"),
            None => str.to_string(),
        }
    };

    let transform_component_dir = |str: &str| -> anyhow::Result<String> {
        Ok(match &ctx.component_dir {
            Some(component_dir) => str.replace("componentDir", fs::path_to_str(component_dir)?),
            None => str.to_string(),
        })
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
            Transform::ComponentDir => transform_component_dir(&transformed)?,
            Transform::ComponentName => transform_component(&transformed),
            Transform::ManifestHints => transform_manifest_hints(&transformed),
            Transform::TsSdk => transform_ts_sdk(&transformed),
            Transform::RustSdk => transform_rust_sdk(&transformed),
            Transform::ApplicationName => transform_app_name(&transformed),
        };
    }

    Ok(transformed)
}

fn transform_file_name(
    ctx: &GeneratorContext<'_>,
    file_name: impl AsRef<str>,
) -> anyhow::Result<String> {
    Ok(transform(ctx, file_name, &[Transform::ComponentName])?
        .replace("Cargo.toml._", "Cargo.toml"))
}

fn transform_target_file_path(
    ctx: &GeneratorContext<'_>,
    target_path: &Path,
) -> anyhow::Result<PathBuf> {
    match ctx.component_dir {
        Some(component_dir) => {
            let relative_target_path = fs::strip_prefix_or_err(target_path, ctx.application_dir)?;
            let transformed_relative_target_path = {
                if component_dir == Path::new(".") {
                    fs::path_to_str(relative_target_path)?.replace("component-dir", "")
                } else {
                    fs::path_to_str(relative_target_path)?
                        .replace("component-dir", fs::path_to_str(component_dir)?)
                }
            };

            Ok(PathBuf::from(format!(
                "{}/{}",
                fs::path_to_str(ctx.application_dir)?,
                transformed_relative_target_path
            )))
        }
        None => Ok(target_path.to_path_buf()),
    }
}

fn get_contents<'a>(dir: &Dir<'a>, source: &'a Path) -> anyhow::Result<&'a str> {
    dir.get_file(source)
        .ok_or_else(|| anyhow!("Could not find entry {}", source.display()))?
        .contents_utf8()
        .ok_or_else(|| anyhow!("File contents are not valid UTF-8: {}", source.display()))
}

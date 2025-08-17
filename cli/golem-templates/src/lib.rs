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
    ComposableAppGroupName, GuestLanguage, PackageName, TargetExistsResolveDecision,
    TargetExistsResolveMode, Template, TemplateKind, TemplateMetadata, TemplateName,
    TemplateParameters,
};
use anyhow::Context;
use include_dir::{include_dir, Dir, DirEntry};
use indoc::indoc;
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

pub mod model;

#[cfg(test)]
test_r::enable!();

static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");
static WIT: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/wit/deps");

static APP_MANIFEST_HEADER: &str = indoc! {"
# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.2.4/golem.schema.json
# Schema for vscode-yaml
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.2.4/golem.schema.json

# See https://learn.golem.cloud/docs/app-manifest#field-reference for field reference
# For creating APIs see https://learn.golem.cloud/invoke/making-custom-apis
"};

static APP_MANIFEST_COMPONENT_HINTS_TEMPLATE: &str = indoc! {"
# Example for adding dependencies for Worker to Worker communication:
# See https://learn.golem.cloud/docs/app-manifest#fields_dependencies for more information
#
#dependencies:
#  componentname:
#  - target: <target component name to be called>
#    type: wasm-rpc
"};

fn all_templates() -> Vec<Template> {
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

                            result.push(template);
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
    all_templates()
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

    for template in all_templates() {
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
    package_name: &PackageName,
) -> anyhow::Result<()> {
    let parameters = TemplateParameters {
        component_name: package_name.to_string_with_colon().into(),
        package_name: package_name.clone(),
        target_path: target_path.into(),
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
        TransformMode::PackageAndComponentOnly,
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
        if !template.exclude.contains(name) && (name != "metadata.json") {
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
                    // TODO: solve this more nicely, for now golem.yaml-s are always transformed,
                    //       even if transform is set to false
                    let transform = if file
                        .path()
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        == "golem.yaml"
                    {
                        if template.kind.is_common() {
                            Some(TransformMode::ManifestHintsOnly)
                        } else {
                            Some(TransformMode::All)
                        }
                    } else {
                        (template.transform && !template.transform_exclude.contains(&name))
                            .then_some(TransformMode::PackageAndComponentOnly)
                    };

                    instantiate_file(
                        catalog,
                        file.path(),
                        &target.join(&name),
                        parameters,
                        transform,
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
    transform_contents: Option<TransformMode>,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    match get_resolved_contents(catalog, source, target, resolve_mode)? {
        Some(contents) => {
            if let Some(transform_mode) = transform_contents {
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
                        transform_mode,
                    ),
                )
            } else {
                fs::write(target, contents)
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

enum TransformMode {
    All,
    PackageAndComponentOnly,
    ManifestHintsOnly,
}

fn transform(str: impl AsRef<str>, parameters: &TemplateParameters, mode: TransformMode) -> String {
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
            .replace(
                "# golem-app-manifest-component-hints\n",
                &transform(
                    APP_MANIFEST_COMPONENT_HINTS_TEMPLATE,
                    parameters,
                    TransformMode::PackageAndComponentOnly,
                ),
            )
    };

    match mode {
        TransformMode::All => transform_manifest_hints(&transform_pack_and_comp(str.as_ref())),
        TransformMode::PackageAndComponentOnly => transform_pack_and_comp(str.as_ref()),
        TransformMode::ManifestHintsOnly => transform_manifest_hints(str.as_ref()),
    }
}

fn file_name_transform(str: impl AsRef<str>, parameters: &TemplateParameters) -> String {
    transform(str, parameters, TransformMode::PackageAndComponentOnly)
        .replace("Cargo.toml._", "Cargo.toml") // HACK because cargo package ignores every subdirectory containing a Cargo.toml
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
        if segments.len() > 2 && segments[1] == "app" {
            if segments.len() > 3 && segments[2] == "component" {
                segments[3..].join("-").into()
            } else {
                segments[2..].join("-").into()
            }
        } else {
            name.into()
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
    }
    if metadata.requires_wasi.unwrap_or(false) {
        wit_deps.push(PathBuf::from("blobstore"));
        wit_deps.push(PathBuf::from("cli"));
        wit_deps.push(PathBuf::from("clocks"));
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
        exclude: metadata
            .exclude
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect(),
        transform_exclude: metadata
            .transform_exclude
            .map(|te| te.iter().cloned().collect())
            .unwrap_or_default(),
        transform: metadata.transform.unwrap_or(true),
    }
}

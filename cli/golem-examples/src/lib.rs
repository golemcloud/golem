use crate::model::{
    ComponentName, ComposableAppGroupName, Example, ExampleKind, ExampleMetadata, ExampleName,
    ExampleParameters, GuestLanguage, PackageName, TargetExistsResolveDecision,
    TargetExistsResolveMode,
};
use include_dir::{include_dir, Dir, DirEntry};
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

#[cfg(feature = "cli")]
pub mod cli;
pub mod model;

#[cfg(test)]
test_r::enable!();

static EXAMPLES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/examples");
static ADAPTERS: Dir<'_> = include_dir!("$OUT_DIR/golem-wit/adapters");
static WIT: Dir<'_> = include_dir!("$OUT_DIR/golem-wit/wit/deps");

fn all_examples() -> Vec<Example> {
    let mut result: Vec<Example> = vec![];
    for entry in EXAMPLES.entries() {
        if let Some(lang_dir) = entry.as_dir() {
            let lang_dir_name = lang_dir.path().file_name().unwrap().to_str().unwrap();
            if let Some(lang) = GuestLanguage::from_string(lang_dir_name) {
                let adapters_path =
                    Path::new(lang.tier().name()).join("wasi_snapshot_preview1.wasm");

                for sub_entry in lang_dir.entries() {
                    if let Some(example_dir) = sub_entry.as_dir() {
                        let example_dir_name =
                            example_dir.path().file_name().unwrap().to_str().unwrap();
                        if example_dir_name != "INSTRUCTIONS" && !example_dir_name.starts_with('.')
                        {
                            let example = parse_example(
                                lang,
                                lang_dir.path(),
                                Path::new("INSTRUCTIONS"),
                                &adapters_path,
                                example_dir.path(),
                            );
                            result.push(example);
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

pub fn all_standalone_examples() -> Vec<Example> {
    all_examples()
        .into_iter()
        .filter(|example| matches!(example.kind, ExampleKind::Standalone))
        .collect()
}

#[derive(Debug, Default)]
pub struct ComposableAppExample {
    pub common: Option<Example>,
    pub components: Vec<Example>,
}

pub fn all_composable_app_examples(
) -> BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppExample>> {
    let mut examples =
        BTreeMap::<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppExample>>::new();

    fn app_examples<'a>(
        examples: &'a mut BTreeMap<
            GuestLanguage,
            BTreeMap<ComposableAppGroupName, ComposableAppExample>,
        >,
        language: GuestLanguage,
        group: &ComposableAppGroupName,
    ) -> &'a mut ComposableAppExample {
        let groups = examples.entry(language).or_default();
        if !groups.contains_key(group) {
            groups.insert(group.clone(), ComposableAppExample::default());
        }
        groups.get_mut(group).unwrap()
    }

    for example in all_examples() {
        match &example.kind {
            ExampleKind::Standalone => continue,
            ExampleKind::ComposableAppCommon { group, .. } => {
                let common = &mut app_examples(&mut examples, example.language, group).common;
                if let Some(common) = common {
                    panic!(
                        "Multiple common examples were found for {} - {}, example paths: {}, {}",
                        example.language,
                        group,
                        common.example_path.display(),
                        example.example_path.display()
                    );
                }
                *common = Some(example);
            }
            ExampleKind::ComposableAppComponent { group } => {
                app_examples(&mut examples, example.language, group)
                    .components
                    .push(example);
            }
        }
    }

    examples
}

pub fn instantiate_example(
    example: &Example,
    parameters: &ExampleParameters,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<String> {
    instantiate_directory(
        &EXAMPLES,
        &example.example_path,
        &parameters.target_path,
        example,
        parameters,
        resolve_mode,
    )?;
    if let Some(adapter_path) = &example.adapter_source {
        let adapter_dir = {
            parameters
                .target_path
                .join(match &example.adapter_target {
                    Some(target) => target.clone(),
                    None => PathBuf::from("adapters"),
                })
                .join(example.language.tier().name())
        };

        fs::create_dir_all(&adapter_dir)?;
        println!("{:?}", &ADAPTERS.entries().iter().collect::<Vec<_>>());
        copy(
            &ADAPTERS,
            adapter_path,
            &adapter_dir.join(adapter_path.file_name().unwrap().to_str().unwrap()),
            TargetExistsResolveMode::MergeOrSkip,
        )?;
    }
    let wit_deps_targets = {
        match &example.wit_deps_targets {
            Some(paths) => paths
                .iter()
                .map(|path| parameters.target_path.join(path))
                .collect(),
            None => vec![parameters.target_path.join("wit").join("deps")],
        }
    };
    for wit_dep in &example.wit_deps {
        for target_wit_deps in &wit_deps_targets {
            let target = target_wit_deps.join(wit_dep.file_name().unwrap().to_str().unwrap());
            copy_all(&WIT, wit_dep, &target, TargetExistsResolveMode::MergeOrSkip)?;
        }
    }
    Ok(render_example_instructions(example, parameters))
}

pub fn add_component_by_example(
    common_example: Option<&Example>,
    component_example: &Example,
    target_path: &Path,
    package_name: &PackageName,
) -> io::Result<()> {
    let parameters = ExampleParameters {
        component_name: ComponentName::new(package_name.to_string_with_colon()),
        package_name: package_name.clone(),
        target_path: target_path.into(),
    };

    if let Some(common_example) = common_example {
        let skip = {
            if let ExampleKind::ComposableAppCommon {
                skip_if_exists: Some(file),
                ..
            } = &common_example.kind
            {
                target_path.join(file).exists()
            } else {
                false
            }
        };

        if !skip {
            instantiate_example(
                common_example,
                &parameters,
                TargetExistsResolveMode::MergeOrSkip,
            )?;
        }
    }

    instantiate_example(
        component_example,
        &parameters,
        TargetExistsResolveMode::MergeOrFail,
    )?;

    Ok(())
}

pub fn render_example_instructions(example: &Example, parameters: &ExampleParameters) -> String {
    transform(&example.instructions, parameters)
}

fn instantiate_directory(
    catalog: &Dir<'_>,
    source: &Path,
    target: &Path,
    example: &Example,
    parameters: &ExampleParameters,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in catalog
        .get_dir(source)
        .unwrap_or_else(|| panic!("Could not find entry {source:?}"))
        .entries()
    {
        let name = entry.path().file_name().unwrap().to_str().unwrap();
        if !example.exclude.contains(name) && (name != "metadata.json") {
            let name = file_name_transform(name, parameters);
            match entry {
                DirEntry::Dir(dir) => {
                    instantiate_directory(
                        catalog,
                        dir.path(),
                        &target.join(&name),
                        example,
                        parameters,
                        resolve_mode,
                    )?;
                }
                DirEntry::File(file) => {
                    instantiate_file(
                        catalog,
                        file.path(),
                        &target.join(&name),
                        parameters,
                        example.transform && !example.transform_exclude.contains(&name),
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
    parameters: &ExampleParameters,
    transform_contents: bool,
    resolve_mode: TargetExistsResolveMode,
) -> io::Result<()> {
    match get_resolved_contents(catalog, source, target, resolve_mode)? {
        Some(contents) => {
            if transform_contents {
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

fn transform(str: impl AsRef<str>, parameters: &ExampleParameters) -> String {
    str.as_ref()
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
}

fn file_name_transform(str: impl AsRef<str>, parameters: &ExampleParameters) -> String {
    transform(str, parameters).replace("Cargo.toml._", "Cargo.toml") // HACK because cargo package ignores every subdirectory containing a Cargo.toml
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

fn parse_example(
    lang: GuestLanguage,
    lang_path: &Path,
    default_instructions_file_name: &Path,
    adapters_path: &Path,
    example_root: &Path,
) -> Example {
    let raw_metadata = EXAMPLES
        .get_file(example_root.join("metadata.json"))
        .expect("Failed to read metadata JSON")
        .contents();
    let metadata = serde_json::from_slice::<ExampleMetadata>(raw_metadata)
        .expect("Failed to parse metadata JSON");

    let kind = match (metadata.app_common_group, metadata.app_component_group) {
        (None, None) => ExampleKind::Standalone,
        (Some(group), None) => ExampleKind::ComposableAppCommon {
            group: ComposableAppGroupName::from_string(group),
            skip_if_exists: metadata.app_common_skip_if_exists.map(PathBuf::from),
        },
        (None, Some(group)) => ExampleKind::ComposableAppComponent {
            group: ComposableAppGroupName::from_string(group),
        },
        (Some(_), Some(_)) => panic!(
            "Only one of appCommonGroup and appComponentGroup can be specified, example root: {}",
            example_root.display()
        ),
    };

    let instructions = match &kind {
        ExampleKind::Standalone => {
            let instructions_path = match metadata.instructions {
                Some(instructions_file_name) => lang_path.join(instructions_file_name),
                None => lang_path.join(default_instructions_file_name),
            };

            let raw_instructions = EXAMPLES
                .get_file(instructions_path)
                .expect("Failed to read instructions")
                .contents();

            String::from_utf8(raw_instructions.to_vec()).expect("Failed to decode instructions")
        }
        ExampleKind::ComposableAppCommon { .. } => "".to_string(),
        ExampleKind::ComposableAppComponent { .. } => "".to_string(),
    };

    let name = ExampleName::from_string(example_root.file_name().unwrap().to_str().unwrap());

    let mut wit_deps: Vec<PathBuf> = vec![];
    if metadata.requires_golem_host_wit.unwrap_or(false) {
        WIT.dirs()
            .filter(|&dir| dir.path().starts_with("golem"))
            .map(|dir| dir.path())
            .for_each(|path| {
                wit_deps.push(path.to_path_buf());
            });

        wit_deps.push(PathBuf::from("golem-1.x"));
        wit_deps.push(PathBuf::from("wasm-rpc"));
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

    let requires_adapter = metadata
        .requires_adapter
        .unwrap_or(metadata.adapter_target.is_some());

    Example {
        name,
        kind,
        language: lang,
        description: metadata.description,
        example_path: example_root.to_path_buf(),
        instructions,
        adapter_source: {
            if requires_adapter {
                Some(adapters_path.to_path_buf())
            } else {
                None
            }
        },
        adapter_target: metadata.adapter_target.map(PathBuf::from),
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

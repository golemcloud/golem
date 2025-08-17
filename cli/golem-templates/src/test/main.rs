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

use anyhow::bail;
use clap::Parser;
use colored::{ColoredString, Colorize};
use golem_templates::model::{
    ComponentName, ComposableAppGroupName, GuestLanguage, PackageName, TargetExistsResolveMode,
    Template, TemplateParameters,
};
use golem_templates::{
    add_component_by_template, all_composable_app_templates, all_standalone_templates,
    instantiate_template, render_template_instructions,
};
use nanoid::nanoid;
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;
use toml_edit::DocumentMut;

// TODO: let's also drop this, and move this logic to the integration tests, or use the golem-cli directly in the golem-cli integ tests)
// TODO: when moving there, let's add tests for rebuilds too
#[derive(Parser, Debug)]
#[command()]
enum Command {
    Templates {
        // Filter templates by name, checks if the template name contains the filter string
        #[arg(short, long)]
        filter: Option<String>,

        // Skip running instructions
        #[arg(long)]
        skip_instructions: bool,

        // Skip instantiating projects
        #[arg(long)]
        skip_instantiate: bool,

        #[arg(long)]
        target_path: Option<String>,
    },
    App {
        #[arg(long)]
        target_path: Option<String>,

        // Filter for some languages, can be defined multiple times
        #[arg(short, long)]
        language: Vec<GuestLanguage>,
    },
}

pub fn main() -> anyhow::Result<()> {
    match Command::parse() {
        Command::Templates {
            filter,
            skip_instructions,
            skip_instantiate,
            target_path,
        } => {
            let filter = filter
                .as_ref()
                .map(|filter| Regex::from_str(filter.as_str()).expect("failed to compile regex"));
            let results: Vec<(Template, Result<(), String>)> = all_standalone_templates()
                .iter()
                .filter(|template| match &filter {
                    Some(filter) => filter.is_match(template.name.as_str()),
                    None => true,
                })
                .map(|template| {
                    let result =
                        test_template(&target_path, skip_instantiate, skip_instructions, template);
                    if let Err(err) = &result {
                        println!("{}", err.bright_red());
                    }
                    (template.clone(), result)
                })
                .collect();

            println!();
            for result in &results {
                println!(
                    "{}: {}",
                    result.0.name.to_string().bold(),
                    match &result.1 {
                        Ok(_) => "OK".bright_green(),
                        Err(err) =>
                            ColoredString::from(format!("{}\n{}", "Failed".bright_red(), err.red())),
                    }
                )
            }
            println!();

            if results.iter().any(|r| r.1.is_err()) {
                exit(1)
            }

            Ok(())
        }
        Command::App {
            target_path,
            language,
        } => {
            let languages = language.into_iter().collect::<Vec<_>>();
            let alphabet: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];

            let target_path = PathBuf::from(
                target_path.unwrap_or_else(|| "target/examples-test-app".to_string()),
            )
            .join("app-default");

            if target_path.exists() {
                println!("Deleting {}", target_path.display().to_string().blue());
                std::fs::remove_dir_all(&target_path)?;
            }

            let app_templates = all_composable_app_templates();

            let mut used_languages = HashSet::<GuestLanguage>::new();
            for (language, templates) in &app_templates {
                if !languages.is_empty() && !languages.contains(language) {
                    continue;
                }

                println!("Adding components for language {}", language.name().blue());
                used_languages.insert(*language);

                let default_templates = templates.get(&ComposableAppGroupName::default()).unwrap();
                for (name, component_template) in &default_templates.components {
                    for _ in 1..=2 {
                        let component_name = format!(
                            "app:comp-{}-{}-{}",
                            nanoid!(10, &alphabet),
                            language.id(),
                            name,
                        );
                        println!(
                            "Adding component {} ({})",
                            component_name.bright_blue(),
                            language.name().blue()
                        );
                        let package_name = PackageName::from_string(component_name).unwrap();
                        add_component_by_template(
                            default_templates.common.as_ref(),
                            Some(component_template),
                            &target_path,
                            &package_name,
                        )?
                    }
                }
            }

            if used_languages.contains(&GuestLanguage::TypeScript) {
                println!("Installing npm packages with golem-cli");
                golem_cli(&target_path, ["app", "ts-npm-install"])?;
            }

            println!("Building with default profile");
            golem_cli(&target_path, ["app", "build"])?;

            Ok(())
        }
    }
}

pub fn golem_cli<I, S>(dir: &Path, args: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let cwd = std::env::current_dir()?;
    let cli_path = cwd.join("../../target/debug/golem-cli");
    let status = std::process::Command::new(cli_path)
        .args(args)
        .current_dir(dir)
        .status()?;
    if !status.success() {
        bail!("golem-cli exited with status: {}", status)
    }
    Ok(())
}

fn test_template(
    target_path: &Option<String>,
    skip_instantiate: bool,
    skip_instructions: bool,
    template: &Template,
) -> Result<(), String> {
    println!();
    println!(
        "{} {}",
        "Generating and testing:".bold().bright_white(),
        template.name.to_string().blue()
    );

    let target_path = PathBuf::from(
        target_path
            .clone()
            .unwrap_or_else(|| "target/examples-test".to_string()),
    );
    let component_name: ComponentName = format!("{}-comp", template.name).into();
    let package_name =
        PackageName::from_string("golemx:componentx").ok_or("failed to create package name")?;
    let component_path = target_path.join(component_name.as_str());

    println!("Target path: {}", target_path.display().to_string().blue());
    println!("Component name: {}", component_name.as_str().blue());
    println!("Package name: {}", package_name.to_string().blue());
    println!(
        "Component path: {}",
        component_path.display().to_string().blue()
    );

    let template_parameters = TemplateParameters {
        component_name: component_name.clone(),
        package_name,
        target_path: target_path.join(component_name.as_str()),
    };

    let run = |command: &str, args: Vec<&str>| -> Result<(), String> {
        let command_formatted = format!("{} {}", command, args.join(" "));
        let run_failed = |e| format!("{command_formatted} failed: {e}");

        println!(
            "Running {} in {}",
            command_formatted.blue(),
            component_path.display().to_string().blue()
        );
        let status = std::process::Command::new(command)
            .args(args.clone())
            .current_dir(&component_path)
            .status()
            .map_err(|e| run_failed(e.to_string()))?;

        match status.code() {
            Some(0) => Ok(()),
            Some(code) => Err(run_failed(format!("non-zero exit code: {code}"))),
            None => Err(run_failed("terminated".to_string())),
        }
    };

    if skip_instantiate {
        println!("Skipping instantiate")
    } else {
        println!("Instantiating");

        if component_path.exists() {
            println!("Deleting {}", component_path.display().to_string().blue());
            std::fs::remove_dir_all(&component_path)
                .map_err(|e| format!("remove dir all failed: {e}"))?;
        }

        let _ = instantiate_template(
            template,
            &template_parameters,
            TargetExistsResolveMode::Fail,
        )
        .map_err(|e| format!("instantiate failed: {e}"))?;

        add_cargo_workspace(&component_path)?;

        println!("Successfully instantiated the example");
    }

    if skip_instructions {
        println!("Skipping instructions\n");
    } else {
        println!("Executing instructions\n");
        let instructions = render_template_instructions(template, &template_parameters);
        for line in instructions.lines() {
            if line.starts_with("  ") {
                match run("bash", vec!["-c", line]) {
                    Ok(_) => {}
                    Err(err) => return Err(err.to_string()),
                }
            } else {
                println!("> {}", line.magenta())
            }
        }
        println!("Successfully executed instructions\n");
    }

    Ok(())
}

fn add_cargo_workspace(project_root: &Path) -> Result<(), String> {
    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Ok(());
    }

    let mut cargo_toml = fs_extra::file::read_to_string(&cargo_toml_path)
        .map_err(|err| {
            format!(
                "failed to read Cargo.toml ({}): {}",
                &cargo_toml_path.display(),
                err
            )
        })?
        .parse::<DocumentMut>()
        .map_err(|err| {
            format!(
                "failed to parse Cargo.toml: ({}): {}",
                &cargo_toml_path.display(),
                err
            )
        })?;

    cargo_toml["workspace"].or_insert(toml_edit::table());

    fs_extra::file::write_all(&cargo_toml_path, &cargo_toml.to_string()).map_err(|err| {
        format!(
            "failed to write Cargo.toml: ({}):, {}",
            &cargo_toml_path.display(),
            err
        )
    })?;

    println!(
        "Added workspace to Cargo.toml ({})",
        cargo_toml_path.display()
    );

    Ok(())
}

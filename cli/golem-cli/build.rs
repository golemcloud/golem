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

use lenient_bool::LenientBool;
use shadow_rs::{BuildPattern, SdResult, ShadowBuilder};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

static SKIP_ENV_VAR_NAME: &str = "GOLEM_BUILD_SKIP_SHADOW";
const GENERATED_TEMPLATES_DIR_NAME: &str = "templates";
const SKILL_DESTINATION: &str = ".agents/skills";
const SUPPORTED_LANGUAGES: &[&str] = &["ts", "rust", "scala", "moonbit"];

fn main() {
    generate_embedded_templates().unwrap();

    if should_skip() {
        println!("cargo::warning=skipping shadow build");
        return;
    }

    ShadowBuilder::builder()
        .hook(append_write_git_describe_tags_hook)
        .build_pattern(BuildPattern::Lazy)
        .build()
        .unwrap();
}

fn generate_embedded_templates() -> std::io::Result<()> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let templates_src = manifest_dir.join("templates");
    let skills_src = manifest_dir.join("../../golem-skills/skills");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let templates_dest = out_dir.join(GENERATED_TEMPLATES_DIR_NAME);

    println!("cargo::rerun-if-changed={}", templates_src.display());
    println!("cargo::rerun-if-changed={}", skills_src.display());

    if templates_dest.exists() {
        fs::remove_dir_all(&templates_dest)?;
    }

    copy_dir_recursive(&templates_src, &templates_dest)?;

    for language in SUPPORTED_LANGUAGES {
        let common_template_dir = templates_dest.join(language).join("common");
        let skills_dest = common_template_dir.join(SKILL_DESTINATION);
        inject_skills(&skills_src.join("common"), &skills_dest)?;
        inject_skills(&skills_src.join(language), &skills_dest)?;
    }

    Ok(())
}

fn inject_skills(skills_src: &Path, skills_dest: &Path) -> std::io::Result<()> {
    if !skills_src.exists() {
        return Ok(());
    }

    // Each child directory of skills_src is a skill; copy it directly into skills_dest
    for entry in fs::read_dir(skills_src)? {
        let entry = entry?;
        if entry.file_name() == ".gitkeep" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            let dest = skills_dest.join(entry.file_name());
            copy_dir_recursive(&entry.path(), &dest)?;
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".gitkeep" {
            continue;
        }

        let dest_path = dst.join(&file_name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &dest_path)?;
        }
    }

    Ok(())
}

fn should_skip() -> bool {
    let skip_requested = std::env::var(SKIP_ENV_VAR_NAME)
        .ok()
        .and_then(|value| value.parse::<LenientBool>().ok())
        .unwrap_or_default()
        .0;

    if !skip_requested {
        return false;
    }

    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("shadow.rs");
    out.exists()
}

fn append_write_git_describe_tags_hook(file: &File) -> SdResult<()> {
    append_write_git_describe_tags(file)?;
    Ok(())
}

fn append_write_git_describe_tags(mut file: &File) -> SdResult<()> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--match", "v*", "--always"])
        .output()?;

    let version = {
        if !output.status.success() {
            println!("cargo::warning=git describe failed, using fallback version 0.0.0");
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("cargo::warning=git stdout: {line}");
            }
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                println!("cargo::warning=git stderr: {line}");
            }

            "0.0.0".to_string()
        } else {
            let version = String::from_utf8(output.stdout)?.trim().to_string();
            println!("cargo::warning=git describe result: {version}");
            version
        }
    };

    let git_describe_tags = format!(
        r#"#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
pub const GIT_DESCRIBE_TAGS: &str = "{version}";"#
    );
    writeln!(file, "{git_describe_tags}")?;

    Ok(())
}

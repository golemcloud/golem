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

use anyhow::{Context, Result, ensure};
use std::path::Path;
use std::{env, fs};
use wit_bindgen_core::{Files, wit_parser::Resolve};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let wit = args.next().context("usage: go-bindgen <wit> <output>")?;
    let output = args.next().context("usage: go-bindgen <wit> <output>")?;
    ensure!(args.next().is_none(), "usage: go-bindgen <wit> <output>");
    generate(Path::new(&wit), Path::new(&output))
}

fn generate(wit: &Path, output: &Path) -> Result<()> {
    let mut resolve = Resolve {
        all_features: true,
        ..Default::default()
    };
    let (package, _) = resolve.push_path(wit)?;
    let world = resolve.select_world(&[package], None)?;
    let mut files = Files::default();
    wit_bindgen_go::Opts {
        pkg_name: Some("github.com/golemcloud/golem/sdks/go/golem/internal/wit".into()),
        export_pkg_name: Some("github.com/golemcloud/golem/sdks/go/golem/internal/exports".into()),
        format: wit_bindgen_go::Format::False,
        outline_memory: true,
        ..Default::default()
    }
    .build()
    .generate(&mut resolve, world, &mut files)?;

    // Generate successfully before replacing the old bindings.
    if output.exists() {
        fs::remove_dir_all(output)?;
    }
    for (name, contents) in files.iter() {
        let path = output.join(name);
        fs::create_dir_all(path.parent().context("generated file has no parent")?)?;
        fs::write(path, contents)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_runtime_matches_guest_module() {
        let runtime = wit_bindgen_go::remote_pkg_version();
        let go_mod = include_str!("../../../sdks/go/golem/go.mod");
        assert!(
            go_mod.lines().any(|line| line
                .split_whitespace()
                .take(2)
                .eq(runtime.split_whitespace())),
            "the guest module must require the generator runtime: {runtime}"
        );
    }

    #[test]
    fn regenerates_bindings_without_touching_exports() {
        let root = env::temp_dir().join(format!("golem-go-bindgen-{}", std::process::id()));
        let wit = root.join("test.wit");
        let output = root.join("wit");
        let exports = root.join("exports");
        fs::create_dir_all(&exports).unwrap();
        fs::write(exports.join("exports.go"), "hand-written exports").unwrap();
        fs::write(
            &wit,
            "package test:bindings; interface api { record item { label: string, value: u32 } echo: func(values: list<item>) -> list<item>; } world test { import api; export api; }",
        )
        .unwrap();
        generate(&wit, &output).unwrap();
        let binding = output.join("test_bindings_api/wit_bindings.go");
        let before = fs::read(&binding).unwrap();
        let text = String::from_utf8_lossy(&before);
        assert!(text.contains("//go:noinline"));
        let glue = fs::read_to_string(output.join("wit_exports/wit_exports.go")).unwrap();
        assert!(glue.contains("github.com/golemcloud/golem/sdks/go/golem/internal/wit"));
        assert!(glue.contains("github.com/golemcloud/golem/sdks/go/golem/internal/exports"));
        fs::write(output.join("stale.go"), "removed interface").unwrap();
        generate(&wit, &output).unwrap();
        assert_eq!(fs::read(&binding).unwrap(), before);
        assert!(!output.join("stale.go").exists());
        assert!(!output.join("export_test_bindings_api").exists());
        assert_eq!(
            fs::read_to_string(exports.join("exports.go")).unwrap(),
            "hand-written exports"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

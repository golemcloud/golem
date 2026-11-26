use anyhow::Context;
use clap::{Parser, Subcommand};
use golem_size_analysis::collect::{CollectInputs, collect, read_module_map, to_json};
use golem_size_analysis::fixtures::Sdk;
use golem_size_analysis::optimize::optimize_with_wasm_opt;
use golem_size_analysis::report::{ReportOptions, report};
use std::io::Write;
use std::path::PathBuf;

/// Component size measurement and the Rule 2 size breakdown.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Measures one component and prints its size, sections and Rule 2 breakdown as JSON.
    Collect {
        /// ts, effect, scala, rust, moonbit or go.
        #[arg(long)]
        sdk: Sdk,
        /// The file `golem deploy` uploads.
        #[arg(long)]
        wasm: PathBuf,
        /// TS, Effect and Scala: the prebuilt guest (`agent_guest.wasm`).
        #[arg(long)]
        guest: Option<PathBuf>,
        /// TS, Effect and Scala: the bundled JavaScript source.
        #[arg(long)]
        module: Option<PathBuf>,
        /// TS and Effect: the rollup module map, a JSON object of module id to rendered length.
        #[arg(long)]
        module_map: Option<PathBuf>,
        /// Rust, Go and MoonBit: a build of the same source that keeps its `name` section.
        #[arg(long)]
        named_wasm: Option<PathBuf>,
        /// Rust: the component crate; Go and MoonBit: the application's module path.
        #[arg(long)]
        user: Option<String>,
        /// Writes the fine breakdown (twiggy symbols and namespaces) to this file.
        #[arg(long)]
        diagnostics: Option<PathBuf>,
    },
    /// Builds the representative Rust fixtures and writes the local size and retention report.
    Report {
        /// A new, non-existing output directory.
        #[arg(long, default_value = "tmp/component-size-report")]
        out: PathBuf,
        /// Also compare per-core `wasm-opt -Oz` (experimental; needs Binaryen on PATH).
        #[arg(long)]
        optimize: bool,
    },
    /// Runs `wasm-opt -Oz` on every core module of a component or core module and re-embeds each
    /// core in its original place (experimental; needs Binaryen on PATH).
    Optimize {
        #[arg(long)]
        wasm: PathBuf,
        #[arg(long, short)]
        output: PathBuf,
    },
}

fn read(path: &Option<PathBuf>) -> anyhow::Result<Option<Vec<u8>>> {
    path.as_ref()
        .map(|p| std::fs::read(p).with_context(|| format!("could not read {}", p.display())))
        .transpose()
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Collect {
            sdk,
            wasm,
            guest,
            module,
            module_map,
            named_wasm,
            user,
            diagnostics,
        } => {
            let uploaded = std::fs::read(&wasm)
                .with_context(|| format!("could not read {}", wasm.display()))?;
            let (guest, module, named) = (read(&guest)?, read(&module)?, read(&named_wasm)?);
            let inputs = CollectInputs {
                uploaded: &uploaded,
                guest: guest.as_deref(),
                module: module
                    .as_deref()
                    .map(std::str::from_utf8)
                    .transpose()
                    .context("JavaScript module must be UTF-8")?,
                module_map: module_map.as_deref().map(read_module_map).transpose()?,
                named: named.as_deref(),
                user: user.as_deref(),
            };
            let (collected, fine) = collect(sdk, &inputs)?;
            if let Some(path) = diagnostics {
                std::fs::write(&path, to_json(&fine)?)?;
            }
            std::io::stdout().write_all(&to_json(&collected)?)?;
        }
        Command::Report { out, optimize } => {
            let report = report(&ReportOptions {
                root: std::env::current_dir()?,
                out,
                optimize,
            })?;
            writeln!(std::io::stdout(), "Report written to {}", report.display())?;
        }
        Command::Optimize { wasm, output } => {
            let work = output.with_extension("cores");
            let optimized = optimize_with_wasm_opt(&std::fs::read(&wasm)?, &work)?;
            std::fs::write(&output, optimized)?;
        }
    }
    Ok(())
}

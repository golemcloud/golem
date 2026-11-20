//! The explicitly invoked local size report (`cargo make component-size-report`). It builds the
//! retention fixtures and the release-profile matrix with Cargo, so it runs external processes
//! and has no unit tests; the checks it runs are unit-tested in their own modules.

use anyhow::{Context, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::attribution::group_by_owner;
use crate::breakdown::SymbolNaming;
use crate::collect::to_json;
use crate::contract::{GOLEM_GUEST_EXPORTS, check_export_contract, component_contract};
use crate::cores::core_modules;
use crate::optimize::{optimize_with_wasm_opt, validate, world_wit};
use crate::retention::{component_evidence, evaluate_retention};
use crate::sections::{BinaryKind, SizeSummary, binary_kind, section_table, summarize};

const ANALYSIS: &str = "test-components/size-analysis";

/// The retention fixtures the report builds, as (label, package).
const FIXTURES: [(&str, &str); 6] = [
    ("canonical-floor", "retention-canonical-floor"),
    ("empty", "retention-empty"),
    ("tool", "retention-non-reflective"),
    ("reflection", "retention-reflection"),
    ("tool-client", "retention-tool-client"),
    ("tool-client-refined", "retention-tool-client-refined"),
];

pub struct ReportOptions {
    /// The repository root.
    pub root: PathBuf,
    /// A new directory for the report.
    pub out: PathBuf,
    /// Also run the experimental per-core `wasm-opt -Oz` comparison (needs Binaryen on PATH).
    pub optimize: bool,
}

fn log(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

fn run(command: &mut Command) -> anyhow::Result<Vec<u8>> {
    log(&format!("+ {command:?}"));
    let output = command
        .output()
        .with_context(|| format!("could not run {command:?}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(4000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        bail!("command failed ({}): {command:?}\n{tail}", output.status);
    }
    Ok(output.stdout)
}

fn on_path(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn version(tool: &str) -> String {
    Command::new(tool)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or("unavailable".to_string(), |o| {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        })
}

fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    std::fs::write(path, to_json(value)?)
        .with_context(|| format!("could not write {}", path.display()))
}

/// Builds one package with Cargo for `wasm32-wasip2` and copies its WASM to
/// `directory/component.wasm`. `overrides` are `CARGO_PROFILE_*` settings, recorded with the
/// command.
fn build(
    manifest: &Path,
    package: Option<&str>,
    directory: &Path,
    overrides: &BTreeMap<String, String>,
) -> anyhow::Result<PathBuf> {
    let located = run(Command::new("cargo")
        .args([
            "locate-project",
            "--workspace",
            "--message-format",
            "plain",
            "--manifest-path",
        ])
        .arg(manifest))?;
    let workspace = PathBuf::from(String::from_utf8(located)?.trim());
    let workspace = workspace
        .parent()
        .context("workspace manifest has no parent")?;
    std::fs::copy(workspace.join("Cargo.lock"), directory.join("Cargo.lock"))?;

    let mut command = Command::new("cargo");
    command
        .args(["build", "--locked", "--manifest-path"])
        .arg(manifest)
        .args([
            "--target",
            "wasm32-wasip2",
            "--profile",
            "release",
            "--message-format=json-render-diagnostics",
        ])
        .envs(overrides);
    if let Some(package) = package {
        command.args(["-p", package]);
    }
    let start = Instant::now();
    let output = run(&mut command)?;
    std::fs::write(directory.join("build.jsonl"), &output)?;
    let mut artifacts = std::collections::BTreeSet::new();
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let message: Value = serde_json::from_slice(line)?;
        if message["reason"] == "compiler-artifact" {
            for file in message["filenames"].as_array().into_iter().flatten() {
                if let Some(file) = file.as_str().filter(|f| f.ends_with(".wasm")) {
                    artifacts.insert(PathBuf::from(file));
                }
            }
        }
    }
    ensure!(
        artifacts.len() == 1,
        "expected one WASM artifact; select a package (found {artifacts:?})"
    );
    let path = directory.join("component.wasm");
    std::fs::copy(artifacts.first().expect("one artifact"), &path)?;
    write_json(
        &directory.join("build-settings.json"),
        &json!({
            "command": format!("{command:?}"),
            "overrides": overrides,
            "seconds": start.elapsed().as_secs_f64(),
        }),
    )?;
    Ok(path)
}

/// Writes the section table, summary, extracted cores, their attribution and the world's WIT of
/// `wasm` into a new `directory`, and returns the summary.
fn analyze(wasm: &Path, directory: &Path) -> anyhow::Result<SizeSummary> {
    std::fs::create_dir_all(directory)?;
    let data = std::fs::read(wasm)?;
    validate(&data)?;
    let rows = section_table(&data)?;
    ensure!(rows.iter().map(|r| r.bytes).sum::<u64>() == data.len() as u64);
    write_json(&directory.join("sections.json"), &rows)?;
    let summary = summarize(&data, &rows);
    write_json(&directory.join("summary.json"), &summary)?;
    for (index, core) in core_modules(&data)?.iter().enumerate() {
        let target = directory.join(format!("core-{index}"));
        std::fs::create_dir(&target)?;
        std::fs::write(target.join("module.wasm"), core)?;
    }
    // Attribution is optional: symbol-stripped builds have no names.
    match crate::attribution::component_symbols(&data) {
        Ok(cores) => {
            for (index, (_, symbols)) in cores.into_iter().enumerate() {
                let target = directory.join(format!("core-{index}"));
                if let Some(symbols) = symbols {
                    write_json(&target.join("symbols.json"), &symbols)?;
                    let mut groups: Vec<(String, u64)> =
                        group_by_owner(&symbols, SymbolNaming::Rust)
                            .into_iter()
                            .collect();
                    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    write_json(&target.join("crate-estimates.json"), &groups)?;
                }
            }
        }
        Err(error) => std::fs::write(
            directory.join("attribution-unavailable.txt"),
            format!("{error:#}\n"),
        )?,
    }
    if binary_kind(&data)? == BinaryKind::Component {
        std::fs::write(directory.join("world.wit"), world_wit(&data)?)?;
    }
    Ok(summary)
}

/// The release-profile matrix: a fixed baseline, then one choice changed at a time.
fn matrix_variants() -> BTreeMap<&'static str, BTreeMap<String, String>> {
    let baseline: BTreeMap<String, String> = [
        ("OPT_LEVEL", "s"),
        ("LTO", "true"),
        ("CODEGEN_UNITS", "16"),
        ("PANIC", "unwind"),
        ("STRIP", "debuginfo"),
        ("DEBUG", "false"),
    ]
    .into_iter()
    .map(|(k, v)| (format!("CARGO_PROFILE_RELEASE_{k}"), v.to_string()))
    .collect();
    let mut variants = BTreeMap::from([("baseline-s", baseline.clone())]);
    for (label, key, value) in [
        ("strip", "STRIP", "symbols"),
        ("cgu1", "CODEGEN_UNITS", "1"),
        ("abort", "PANIC", "abort"),
        ("z", "OPT_LEVEL", "z"),
    ] {
        let mut overrides = baseline.clone();
        overrides.insert(format!("CARGO_PROFILE_RELEASE_{key}"), value.to_string());
        variants.insert(label, overrides);
    }
    variants
}

fn optimize_file(wasm: &Path, output: &Path) -> anyhow::Result<()> {
    let work = output
        .parent()
        .context("output has a parent")?
        .join("optimization");
    std::fs::write(
        output,
        optimize_with_wasm_opt(&std::fs::read(wasm)?, &work)?,
    )?;
    Ok(())
}

pub fn report(options: &ReportOptions) -> anyhow::Result<PathBuf> {
    let root = &options.root;
    let analysis = root.join(ANALYSIS);
    ensure!(
        analysis.is_dir(),
        "run the report from the repository root ({} not found)",
        analysis.display()
    );
    ensure!(
        !options.out.exists(),
        "output directory already exists: {}",
        options.out.display()
    );
    let mut required = vec!["cargo", "rustc"];
    if options.optimize {
        required.push("wasm-opt");
    }
    let missing: Vec<&str> = required.into_iter().filter(|tool| !on_path(tool)).collect();
    ensure!(
        missing.is_empty(),
        "missing required tools: {}",
        missing.join(", ")
    );
    std::fs::create_dir_all(&options.out)?;
    let out = options.out.canonicalize()?;

    let versions: BTreeMap<&str, String> = ["rustc", "cargo", "wasm-opt", "node"]
        .into_iter()
        .map(|tool| (tool, version(tool)))
        .collect();
    write_json(&out.join("versions.json"), &versions)?;

    let fixture_manifest = analysis.join("fixtures/Cargo.toml");
    let mut fixtures = BTreeMap::new();
    let mut fixture_wasm = BTreeMap::new();
    for (label, package) in FIXTURES {
        let directory = out.join("retention").join(label).join("current");
        std::fs::create_dir_all(&directory)?;
        let wasm = build(
            &fixture_manifest,
            Some(package),
            &directory,
            &BTreeMap::new(),
        )?;
        fixtures.insert(label, analyze(&wasm, &directory.join("report"))?);
        fixture_wasm.insert(label, std::fs::read(&wasm)?);
    }

    check_export_contract(
        &component_contract(&fixture_wasm["canonical-floor"])?,
        &component_contract(&fixture_wasm["empty"])?,
        &GOLEM_GUEST_EXPORTS,
    )?;
    log("five-interface export contract unchanged; no new imports");

    let retention = evaluate_retention(
        &BTreeMap::from([
            (
                "empty".to_string(),
                component_evidence(&fixture_wasm["empty"])?,
            ),
            (
                "tool".to_string(),
                component_evidence(&fixture_wasm["tool"])?,
            ),
        ]),
        &BTreeMap::from([(
            "reflection".to_string(),
            component_evidence(&fixture_wasm["reflection"])?,
        )]),
        true,
    )?;
    write_json(&out.join("reflection-retention.json"), &retention)?;

    let counters = root.join("test-components/agent-counters/Cargo.toml");
    let matrix_dir = out.join("release-profile-matrix");
    let mut matrix = BTreeMap::new();
    let mut benchmark_cores = Vec::new();
    for (label, overrides) in matrix_variants() {
        let directory = matrix_dir.join(label);
        std::fs::create_dir_all(&directory)?;
        let wasm = build(&counters, None, &directory, &overrides)?;
        matrix.insert(
            label.to_string(),
            analyze(&wasm, &directory.join("report"))?,
        );
        if label != "abort" {
            benchmark_cores.push(directory.join("report/core-0/module.wasm"));
        }
        if options.optimize {
            let optimized = directory.join("optimized.wasm");
            optimize_file(&wasm, &optimized)?;
            matrix.insert(
                format!("{label}-Oz"),
                analyze(&optimized, &directory.join("optimized-report"))?,
            );
            if label == "strip" {
                benchmark_cores.push(directory.join("optimized-report/core-0/module.wasm"));
            }
        }
    }
    write_json(&matrix_dir.join("summary.json"), &matrix)?;

    let allocator = if on_path("node") {
        let output = run(Command::new("node")
            .arg(analysis.join("benchmark-core.mjs"))
            .args(&benchmark_cores))?;
        std::fs::write(out.join("allocator-performance.json"), &output)?;
        serde_json::from_slice(&output)?
    } else {
        Value::Null
    };

    let details = json!({
        "versions": versions,
        "fixtures": fixtures,
        "release_profile_matrix": matrix,
        "reflection_retention": retention,
        "allocator_performance": allocator,
    });
    write_json(&out.join("report.json"), &details)?;

    let row = |label: &str, s: &SizeSummary| {
        format!(
            "| {label} | {} | {} | {} | {} | {} |",
            s.bytes, s.gzip_bytes, s.code, s.data, s.custom
        )
    };
    let header = [
        "| Fixture | Component bytes | Gzip bytes | Code bytes | Data bytes | Custom bytes |",
        "|---|---:|---:|---:|---:|---:|",
    ];
    let mut lines = vec![
        "# Component size report".to_string(),
        String::new(),
        "Comparisons are meaningful only with the same toolchain, lockfile, Cargo configuration, and environment.".to_string(),
        String::new(),
        "## Retention fixtures".to_string(),
        String::new(),
    ];
    lines.extend(header.map(String::from));
    lines.extend(fixtures.iter().map(|(label, s)| row(label, s)));
    lines.extend([
        "".to_string(),
        "## Release profile matrix".to_string(),
        String::new(),
    ]);
    lines.extend(header.map(|h| h.replace("| Fixture |", "| Variant |")));
    lines.extend(matrix.iter().map(|(label, s)| row(label, s)));
    lines.extend([
        String::new(),
        "Reflection/model retention contracts passed; the export contract of the empty fixture matches the canonical floor.".to_string(),
        if allocator.is_null() { "Node was not on PATH, so the allocator smoke benchmark did not run.".to_string() } else { "The allocator smoke benchmark is in `allocator-performance.json`.".to_string() },
        "See `report.json` and the per-component directories for machine-readable details, symbols, sections, WIT, Cargo settings, and artifacts.".to_string(),
        String::new(),
    ]);
    let report = out.join("report.md");
    std::fs::write(&report, lines.join("\n"))?;
    Ok(report)
}

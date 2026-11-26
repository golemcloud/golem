use anyhow::{Context, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::attribution::{Symbol, group_by_owner};
use crate::breakdown::{
    Breakdown, JsComponent, SymbolClassifier, js_breakdown, rust_breakdown, scala_breakdown,
    scaled_breakdown,
};
use crate::fixtures::Sdk;
use crate::sections::{SectionRow, SizeSummary, section_table, summarize};

/// The inputs of one measurement.
#[derive(Debug, Clone, Default)]
pub struct CollectInputs<'a> {
    /// The file `golem deploy` uploads.
    pub uploaded: &'a [u8],
    /// TS, Effect and Scala: the prebuilt guest and the bundled JavaScript source.
    pub guest: Option<&'a [u8]>,
    pub module: Option<&'a str>,
    /// TS and Effect: the rollup module map, module id to rendered length.
    pub module_map: Option<BTreeMap<String, u64>>,
    /// Rust, Go and MoonBit: a build of the same source that keeps its `name` section. Without
    /// it, Rust attributes the uploaded file itself, which must then be named.
    pub named: Option<&'a [u8]>,
    /// Rust: the component crate; Go and MoonBit: the application's module path.
    pub user: Option<&'a str>,
}

/// The published measurement of one component. Serializing the same inputs gives the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Collected {
    pub sdk: Sdk,
    pub summary: SizeSummary,
    pub sections: Vec<SectionRow>,
    pub breakdown: Breakdown,
}

/// The fine breakdown behind [`Collected`]: twiggy symbols and their namespaces. It is a
/// diagnostic artifact, not part of the published series.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Diagnostics {
    pub symbols: Vec<Symbol>,
    pub namespaces: BTreeMap<String, u64>,
}

pub fn collect(sdk: Sdk, inputs: &CollectInputs<'_>) -> anyhow::Result<(Collected, Diagnostics)> {
    let sections = section_table(inputs.uploaded)?;
    let summary = summarize(inputs.uploaded, &sections);
    let mut naming = None;
    let (breakdown, symbols) = match sdk {
        Sdk::Ts | Sdk::Effect | Sdk::Scala => {
            let guest = inputs.guest.context("JS components need --guest")?;
            let module = inputs.module.context("JS components need --module")?;
            let reference = tempfile::tempdir()?;
            let input = reference.path().join("guest.wasm");
            let output = reference.path().join("injected.wasm");
            std::fs::write(&input, guest)?;
            wasm_rquickjs::inject_js_into_component(
                camino::Utf8Path::from_path(&input).context("Non-UTF-8 reference path")?,
                camino::Utf8Path::from_path(&output).context("Non-UTF-8 reference path")?,
                &[module],
            )?;
            let dynamic = std::fs::read(&output)?;
            let component = JsComponent {
                guest,
                dynamic: &dynamic,
                uploaded: inputs.uploaded,
            };
            let breakdown = if sdk == Sdk::Scala {
                scala_breakdown(&component)?
            } else {
                let module_map = inputs
                    .module_map
                    .as_ref()
                    .context("TS and Effect components need --module-map")?;
                js_breakdown(&component, module_map)?
            };
            (breakdown, Vec::new())
        }
        Sdk::Rust | Sdk::Go | Sdk::Moonbit => {
            let user = inputs.user.context("compiled SDKs need --user")?;
            let classifier = match sdk {
                Sdk::Rust => SymbolClassifier::rust(&user.replace('-', "_")),
                Sdk::Go => SymbolClassifier::go(user),
                _ => SymbolClassifier::moonbit(user),
            };
            naming = Some(classifier.naming);
            match (sdk, inputs.named) {
                (Sdk::Rust, None) => rust_breakdown(inputs.uploaded, &classifier)?,
                (_, Some(named)) if named == inputs.uploaded => rust_breakdown(named, &classifier)?,
                (_, Some(named)) => scaled_breakdown(named, inputs.uploaded, &classifier)?,
                (_, None) => {
                    bail!("Go and MoonBit release builds have no names; pass --named-wasm")
                }
            }
        }
    };
    let namespaces = naming.map_or_else(BTreeMap::new, |naming| group_by_owner(&symbols, naming));
    Ok((
        Collected {
            sdk,
            summary,
            sections,
            breakdown,
        },
        Diagnostics {
            symbols,
            namespaces,
        },
    ))
}

/// Reads a rollup module map: a JSON object of module id to rendered length.
pub fn read_module_map(path: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    serde_json::from_slice(&std::fs::read(path)?).with_context(|| {
        format!(
            "{} is not a JSON object of module id to bytes",
            path.display()
        )
    })
}

/// Pretty JSON with a trailing newline, the form every artifact of this crate is written in.
pub fn to_json(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    let mut json = serde_json::to_vec_pretty(value)?;
    json.push(b'\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::Bucket;
    use test_r::test;

    #[test]
    fn js_collection_uses_template_and_source_without_a_dynamic_build() {
        let marker: String = wasm_rquickjs::create_marker_file(0)
            .iter()
            .map(|byte| format!("\\{byte:02x}"))
            .collect();
        let guest = wat::parse_str(format!(
            "(component (core module (memory 1) (data (i32.const 0) \"{marker}\")))"
        ))
        .unwrap();
        let uploaded = wat::parse_str(format!(
            "(component (core module (memory 1) (data (i32.const 0) \"{}\")))",
            "x".repeat(2048)
        ))
        .unwrap();
        let source = "export const x = 1;";
        let inputs = CollectInputs {
            guest: Some(&guest),
            module: Some(source),
            uploaded: &uploaded,
            module_map: Some(BTreeMap::from([(
                "src/main.ts".into(),
                source.len() as u64,
            )])),
            ..Default::default()
        };
        for sdk in [Sdk::Ts, Sdk::Effect, Sdk::Scala] {
            let (collected, _) = collect(sdk, &inputs).unwrap();
            let bundle = if sdk == Sdk::Scala {
                Bucket::Bundle
            } else {
                Bucket::UserCode
            };
            // Source payload: 4 length bytes + 19 source bytes, plus 7 segment framing bytes.
            assert_eq!(collected.breakdown.bucket(bundle), 30);
            // Uploaded data: 2058 bytes; reference: 48 original + 30 injected bytes.
            assert_eq!(collected.breakdown.bucket(Bucket::Snapshot), 2058 - 78);
            assert_eq!(collected.breakdown.bytes, uploaded.len() as u64);
        }
    }

    #[test]
    fn quick_collection_matches_real_injection_across_source_size_boundaries() {
        let marker: String = wasm_rquickjs::create_marker_file(0)
            .iter()
            .map(|byte| format!("\\{byte:02x}"))
            .collect();
        let guest = wat::parse_str(format!(
            "(component (core module (memory 1) (data (i32.const 0) \"{marker}\")))"
        ))
        .unwrap();
        for length in [0, 1, 18, 19, 126, 127, 128, 16383, 16384, 65536] {
            let source = " ".repeat(length);
            let directory = tempfile::tempdir().unwrap();
            let input = directory.path().join("guest.wasm");
            let output = directory.path().join("quick.wasm");
            std::fs::write(&input, &guest).unwrap();
            wasm_rquickjs::inject_js_into_component(
                camino::Utf8Path::from_path(&input).unwrap(),
                camino::Utf8Path::from_path(&output).unwrap(),
                &[&source],
            )
            .unwrap();
            let injected = std::fs::read(&output).unwrap();
            // The original data section is 48 bytes. The added active segment has a
            // one-byte flag, five-byte offset expression, and a length-prefixed payload.
            let leb_bytes =
                |n: usize| (usize::BITS - n.leading_zeros()).max(1).div_ceil(7) as usize;
            let payload = 4 + length;
            let data_payload = 1 + 45 + 1 + 5 + leb_bytes(payload) + payload;
            let bundle_bytes = (1 + leb_bytes(data_payload) + data_payload - 48) as u64;
            for sdk in [Sdk::Ts, Sdk::Effect, Sdk::Scala] {
                let inputs = CollectInputs {
                    uploaded: &injected,
                    guest: Some(&guest),
                    module: Some(&source),
                    module_map: Some(BTreeMap::from([("src/main.ts".into(), 1)])),
                    ..Default::default()
                };
                let (collected, _) = collect(sdk, &inputs).unwrap();
                assert_eq!(collected.breakdown.bucket(Bucket::Snapshot), 0);
                assert_eq!(
                    collected.breakdown.bucket(Bucket::Runtime),
                    injected.len() as u64 - bundle_bytes
                );
                let bundle = if sdk == Sdk::Scala {
                    Bucket::Bundle
                } else {
                    Bucket::UserCode
                };
                assert_eq!(collected.breakdown.bucket(bundle), bundle_bytes);
                assert_eq!(collected.breakdown.bytes, injected.len() as u64);
            }
        }
    }
}

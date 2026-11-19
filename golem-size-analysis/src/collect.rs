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
    /// TS, Effect and Scala: the prebuilt guest and the `.dynamic.wasm` component.
    pub guest: Option<&'a [u8]>,
    pub dynamic: Option<&'a [u8]>,
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
            let component = JsComponent {
                guest: inputs.guest.context("JS components need --guest")?,
                dynamic: inputs.dynamic.context("JS components need --dynamic")?,
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

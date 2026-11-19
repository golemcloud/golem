use anyhow::{Context, ensure};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::attribution::component_symbols;

/// Host imports that only reflection needs, and the interface each belongs to.
pub const REFLECTION_IMPORTS: [(&str, &str); 3] = [
    ("get-agent-type", "golem:agent/host"),
    ("get-all-agent-types", "golem:agent/host"),
    ("get-tool-type", "golem:tool/host"),
];

/// Model and reflection helpers that a component without reflection should not retain.
pub const OPTION_B_SYMBOLS: [&str; 9] = [
    "golem_rust::agentic::reflection",
    "golem_rust::agentic::tool_reflection",
    "golem_schema::schema::render",
    "golem_schema::schema::validation",
    "golem_schema::schema::well_formedness",
    "regex_automata",
    "url::",
    "idna",
    "serde_json::value",
];

/// What a component retains: the text of every core module and the demangled name of every
/// symbol of its named core modules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    pub wat: String,
    pub symbols: String,
}

pub fn component_evidence(component: &[u8]) -> anyhow::Result<Evidence> {
    let mut evidence = Evidence::default();
    for (core, symbols) in component_symbols(component)? {
        evidence.wat +=
            &wasmprinter::print_bytes(&core).context("could not print a core module")?;
        if let Some(symbols) = symbols {
            for symbol in symbols {
                evidence.symbols += &symbol.name;
                evidence.symbols.push('\n');
            }
        }
    }
    Ok(evidence)
}

static DISAMBIGUATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\[[0-9a-f]+\]").expect("valid regex"));

static REFLECTION_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    let names: Vec<String> = REFLECTION_IMPORTS
        .iter()
        .map(|(name, _)| regex::escape(name))
        .collect();
    Regex::new(&format!(
        r#"\(import\s+"([^"]+)"\s+"({})""#,
        names.join("|")
    ))
    .expect("valid regex")
});

fn retained_patterns(text: &str, patterns: &[&str]) -> Vec<String> {
    let normalized = DISAMBIGUATOR.replace_all(text, "$1");
    patterns
        .iter()
        .filter(|pattern| normalized.contains(*pattern))
        .map(|pattern| pattern.to_string())
        .collect()
}

/// The reflection imports a core module imports from their own interface (any version).
fn retained_reflection_imports(wat: &str) -> Vec<String> {
    let imported: BTreeSet<&str> = REFLECTION_IMPORT
        .captures_iter(wat)
        .filter_map(|captures| {
            let (interface, name) = (captures.get(1)?.as_str(), captures.get(2)?.as_str());
            let expected = REFLECTION_IMPORTS.iter().find(|(n, _)| *n == name)?.1;
            (interface.split('@').next() == Some(expected)).then_some(name)
        })
        .collect();
    REFLECTION_IMPORTS
        .iter()
        .filter(|(name, _)| imported.contains(name))
        .map(|(name, _)| name.to_string())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegativeReport {
    pub forbidden_reflection_imports: Vec<String>,
    pub pending_option_b_negative_assertions: Vec<String>,
    pub pending_option_b_symbols_still_retained: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositiveReport {
    pub expected_reflection_imports: Vec<String>,
    pub expected_reflection_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionReport {
    pub negative: BTreeMap<String, NegativeReport>,
    pub positive: BTreeMap<String, PositiveReport>,
}

/// Checks the reflection retention contracts: `negative` components import no reflection host
/// function (and, with `enforce_option_b`, retain no model or reflection helper); `positive`
/// components import `get-agent-type` and retain the fixture's `has_agent_type` method.
pub fn evaluate_retention(
    negative: &BTreeMap<String, Evidence>,
    positive: &BTreeMap<String, Evidence>,
    enforce_option_b: bool,
) -> anyhow::Result<RetentionReport> {
    let mut failures = Vec::new();
    let mut report = RetentionReport {
        negative: BTreeMap::new(),
        positive: BTreeMap::new(),
    };
    for (label, evidence) in negative {
        let imports = retained_reflection_imports(&evidence.wat);
        let pending = retained_patterns(&evidence.symbols, &OPTION_B_SYMBOLS);
        if !imports.is_empty() {
            failures.push(format!(
                "{label} retains reflection imports: {}",
                imports.join(", ")
            ));
        }
        if enforce_option_b && !pending.is_empty() {
            failures.push(format!(
                "{label} retains option B symbols: {}",
                pending.join(", ")
            ));
        }
        report.negative.insert(
            label.clone(),
            NegativeReport {
                forbidden_reflection_imports: imports,
                pending_option_b_negative_assertions: OPTION_B_SYMBOLS.map(String::from).to_vec(),
                pending_option_b_symbols_still_retained: pending,
            },
        );
    }
    for (label, evidence) in positive {
        let imports = retained_reflection_imports(&evidence.wat);
        let symbols = retained_patterns(&evidence.symbols, &["has_agent_type"]);
        if !imports.iter().any(|i| i == "get-agent-type") {
            failures.push(format!("{label} does not retain get-agent-type"));
        }
        if symbols.is_empty() {
            failures.push(format!(
                "{label} does not retain the explicit reflection fixture symbol"
            ));
        }
        report.positive.insert(
            label.clone(),
            PositiveReport {
                expected_reflection_imports: imports,
                expected_reflection_symbols: symbols,
            },
        );
    }
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn evidence(wat: &str, symbols: &str) -> Evidence {
        Evidence {
            wat: wat.to_string(),
            symbols: symbols.to_string(),
        }
    }

    fn one(label: &str, evidence: Evidence) -> BTreeMap<String, Evidence> {
        BTreeMap::from([(label.to_string(), evidence)])
    }

    fn reflection() -> BTreeMap<String, Evidence> {
        one(
            "reflection",
            evidence(
                r#"(module (import "golem:agent/host@2.0.0" "get-agent-type" (func)))"#,
                "retention_reflection[1a2b]::has_agent_type",
            ),
        )
    }

    #[test]
    fn current_and_option_b_expectations_are_reported_separately() {
        let report = evaluate_retention(
            &one(
                "tool",
                evidence(
                    r#"(import "golem:agent/host" "make-agent-id")"#,
                    &OPTION_B_SYMBOLS.join("\n"),
                ),
            ),
            &reflection(),
            false,
        )
        .unwrap();
        assert_eq!(
            report.negative["tool"].pending_option_b_symbols_still_retained,
            OPTION_B_SYMBOLS.map(String::from).to_vec()
        );
        assert_eq!(
            report.positive["reflection"].expected_reflection_imports,
            vec!["get-agent-type"]
        );
    }

    #[test]
    fn wrong_fixture_polarity_fails() {
        let error = evaluate_retention(
            &one(
                "tool",
                evidence(r#"(import "golem:tool/host" "get-tool-type" (func))"#, ""),
            ),
            &reflection(),
            false,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("retains reflection imports"),
            "{error}"
        );

        let error = evaluate_retention(
            &one("tool", Evidence::default()),
            &one("reflection", Evidence::default()),
            false,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("does not retain get-agent-type"),
            "{error}"
        );

        let error = evaluate_retention(
            &one("tool", evidence("", "regex_automata::dfa")),
            &reflection(),
            true,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("retains option B symbols"),
            "{error}"
        );
    }

    #[test]
    fn exports_and_unrelated_interfaces_are_not_reflection_imports() {
        for wat in [
            r#"(module (func (export "get-agent-type")))"#,
            r#"(module (import "example:unrelated/host" "get-agent-type" (func)))"#,
        ] {
            let report =
                evaluate_retention(&one("tool", evidence(wat, "")), &reflection(), false).unwrap();
            assert!(
                report.negative["tool"]
                    .forbidden_reflection_imports
                    .is_empty()
            );
        }
    }

    #[test]
    fn evidence_prints_every_core_and_names_their_symbols() {
        let core = wat::parse_str(
            r#"(module (import "golem:agent/host" "get-agent-type" (func))
                (func $_ZN20retention_reflection14has_agent_type17h0123456789abcdefE
                    (export "f") call 0))"#,
        )
        .unwrap();
        let evidence = component_evidence(&core).unwrap();
        assert!(
            evidence
                .wat
                .contains(r#"(import "golem:agent/host" "get-agent-type""#)
        );
        assert!(
            evidence
                .symbols
                .contains("retention_reflection::has_agent_type")
        );
    }
}

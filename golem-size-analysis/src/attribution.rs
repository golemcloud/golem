use anyhow::{Context, bail};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::breakdown::{SymbolNaming, owner};
use crate::cores::core_modules;
use crate::sections::{CORE_CODE_SECTION, CUSTOM_SECTION, custom_section_name, raw_sections};

/// The bucket of symbols whose name has no crate or package namespace, such as data segments,
/// the `name` section itself and imports.
pub const UNATTRIBUTED: &str = "[unattributed]";

/// One twiggy item of a core module: its demangled name and shallow size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub name: String,
    pub bytes: u64,
}

/// The twiggy items of a core module, largest first, then by name. Returns `None` for a core
/// with no code section (such as a canonical-ABI shim), which has nothing to attribute. Without a
/// `name` section the items are anonymous (`code[N]`); [`component_symbols`] rejects that for the
/// main module.
pub fn symbols(core: &[u8]) -> anyhow::Result<Option<Vec<Symbol>>> {
    if !raw_sections(core)?
        .iter()
        .any(|s| s.id == CORE_CODE_SECTION)
    {
        return Ok(None);
    }
    let items = twiggy_parser::parse(core).context("twiggy could not parse the core module")?;
    let mut symbols: Vec<Symbol> = items
        .iter()
        .map(|item| Symbol {
            name: item.name().to_string(),
            bytes: u64::from(item.size()),
        })
        .collect();
    symbols.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    Ok(Some(symbols))
}

/// Whether a `name` section payload (after the section name) has a function-names subsection.
fn has_function_names(payload: &[u8]) -> anyhow::Result<bool> {
    let mut reader = wasmparser::BinaryReader::new(payload, 0);
    reader
        .read_string()
        .map_err(|error| anyhow::anyhow!("invalid custom section name: {error}"))?;
    let offset = reader.original_position();
    for subsection in wasmparser::NameSectionReader::new(wasmparser::BinaryReader::new(
        &payload[offset..],
        offset,
    )) {
        if matches!(subsection?, wasmparser::Name::Function(_)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The code bytes of a core module and whether its `name` section names its functions.
fn code_bytes_and_names(core: &[u8]) -> anyhow::Result<(usize, bool)> {
    let mut code = 0;
    let mut named = false;
    for section in raw_sections(core)? {
        let payload = &core[section.payload.clone()];
        if section.id == CORE_CODE_SECTION {
            code += section.whole.len();
        } else if section.id == CUSTOM_SECTION
            && custom_section_name(payload)? == "name"
            && has_function_names(payload)?
        {
            named = true;
        }
    }
    Ok((code, named))
}

/// The bytes of the custom sections of every core module of `file`, with their framing, by
/// section name.
pub fn core_custom_section_bytes(file: &[u8]) -> anyhow::Result<BTreeMap<String, u64>> {
    let mut bytes = BTreeMap::new();
    for core in core_modules(file)? {
        for section in raw_sections(&core)? {
            if section.id == CUSTOM_SECTION {
                let name = custom_section_name(&core[section.payload.clone()])?;
                *bytes.entry(name.to_string()).or_default() += section.whole.len() as u64;
            }
        }
    }
    Ok(bytes)
}

/// The custom-section bytes of the cores of `named` that `uploaded` does not carry: for each
/// section name, how much more `named` has. These are the `name` section and the DWARF
/// `.debug_*` sections of a debug build.
pub fn extra_custom_section_bytes(named: &[u8], uploaded: &[u8]) -> anyhow::Result<u64> {
    let uploaded = core_custom_section_bytes(uploaded)?;
    Ok(core_custom_section_bytes(named)?
        .into_iter()
        .map(|(name, bytes)| bytes.saturating_sub(uploaded.get(&name).copied().unwrap_or(0)))
        .sum())
}

/// A core module and its twiggy items (`None` without code).
pub type CoreSymbols = (Vec<u8>, Option<Vec<Symbol>>);

/// Every core module of `file` with its twiggy items. Fails when the core with the most code
/// (the main module) has no function names: a stripped build gives no attribution, and silently
/// putting every byte in [`UNATTRIBUTED`] would hide that. Small generated shims without names
/// are allowed.
pub fn component_symbols(file: &[u8]) -> anyhow::Result<Vec<CoreSymbols>> {
    let cores = core_modules(file)?;
    let mut main: Option<(usize, bool)> = None;
    for core in &cores {
        let (code, named) = code_bytes_and_names(core)?;
        if code > 0 && main.is_none_or(|(largest, _)| code > largest) {
            main = Some((code, named));
        }
    }
    if let Some((_, false)) = main {
        bail!(
            "the main core module has no function names in a `name` section, so its symbols \
             cannot be attributed; \
             measure a build that keeps names (for Rust, `CARGO_PROFILE_RELEASE_STRIP=debuginfo`; \
             for Go and MoonBit, a debug build)"
        );
    }
    cores
        .into_iter()
        .map(|core| {
            let symbols = symbols(&core)?;
            Ok((core, symbols))
        })
        .collect()
}

/// Bytes per owner (crate or package, see [`crate::breakdown::owner`]), as a sorted map so the
/// JSON output is deterministic. Symbols with no owner are [`UNATTRIBUTED`]. After LTO and
/// inlining this is an estimate of ownership, not of retained size.
pub fn group_by_owner(symbols: &[Symbol], naming: SymbolNaming) -> BTreeMap<String, u64> {
    let mut groups = BTreeMap::new();
    for symbol in symbols {
        let owner = owner(naming, &symbol.name).unwrap_or(UNATTRIBUTED.into());
        *groups.entry(owner.into_owned()).or_default() += symbol.bytes;
    }
    groups
}

/// Scales buckets measured on one build (a named debug build of Go or MoonBit) to the size of
/// another file (the release build that is uploaded). Each bucket keeps its share; the largest
/// remainders absorb the rounding, so the result adds up to exactly `target`.
pub fn scale_to<K: Ord + Clone>(buckets: &BTreeMap<K, u64>, target: u64) -> BTreeMap<K, u64> {
    let total: u64 = buckets.values().sum();
    if total == 0 {
        return buckets.keys().map(|k| (k.clone(), 0)).collect();
    }
    let mut scaled: Vec<(&K, u64, u128)> = buckets
        .iter()
        .map(|(name, &bytes)| {
            let exact = u128::from(bytes) * u128::from(target);
            let floor = (exact / u128::from(total)) as u64;
            (name, floor, exact % u128::from(total))
        })
        .collect();
    let mut missing = target - scaled.iter().map(|(_, floor, _)| floor).sum::<u64>();
    let mut order: Vec<usize> = (0..scaled.len()).collect();
    order.sort_by(|&a, &b| {
        scaled[b]
            .2
            .cmp(&scaled[a].2)
            .then_with(|| scaled[a].0.cmp(scaled[b].0))
    });
    for index in order {
        if missing == 0 {
            break;
        }
        scaled[index].1 += 1;
        missing -= 1;
    }
    scaled
        .into_iter()
        .map(|(name, bytes, _)| (name.clone(), bytes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn named_core() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (memory 1)
                (data (i32.const 0) "0123456789abcdef0123456789abcdef")
                (func $_ZN10golem_rust5agent6invoke17h0123456789abcdefE (result i32)
                    i32.const 1 i32.const 2 i32.add i32.const 3 i32.mul)
                (func $_ZN4core3fmt5write17h0123456789abcdefE (result i32) i32.const 7)
                (export "a" (func 0)) (export "b" (func 1)))"#,
        )
        .unwrap()
    }

    #[test]
    fn named_core_attributes_every_byte_to_demangled_namespaces() {
        let core = named_core();
        let symbols = symbols(&core).unwrap().unwrap();
        assert_eq!(
            symbols.iter().map(|s| s.bytes).sum::<u64>(),
            core.len() as u64
        );
        let groups = group_by_owner(&symbols, SymbolNaming::Rust);
        assert!(groups["golem_rust"] > groups["core"], "{groups:?}");
        assert!(groups.contains_key(UNATTRIBUTED));
        assert_eq!(groups.values().sum::<u64>(), core.len() as u64);
    }

    #[test]
    fn stripped_main_module_fails_but_unnamed_shims_are_allowed() {
        let stripped = wat::parse_str(
            "(module (func (export \"f\") (result i32) i32.const 1 i32.const 2 i32.add))",
        )
        .unwrap();
        let error = component_symbols(&stripped).unwrap_err().to_string();
        assert!(error.contains("no function names"), "{error}");

        // A `name` section with only a module name does not name the functions.
        let module_name_only = wat::parse_str(
            "(module $m (func (export \"f\") (result i32) i32.const 1 i32.const 2 i32.add))",
        )
        .unwrap();
        let error = component_symbols(&module_name_only)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no function names"), "{error}");

        let unnamed_shim = wat::parse_str("(module (func (export \"s\")))").unwrap();
        let file = crate::sections::tests::component(&[
            crate::sections::tests::section(1, &named_core()),
            crate::sections::tests::section(1, &unnamed_shim),
            crate::sections::tests::section(1, &wat::parse_str("(module (memory 1))").unwrap()),
        ]);
        let cores = component_symbols(&file).unwrap();
        assert_eq!(cores.len(), 3);
        assert!(
            cores[1]
                .1
                .as_ref()
                .unwrap()
                .iter()
                .any(|s| s.name.starts_with("code["))
        );
        assert_eq!(cores[2].1, None);
    }

    #[test]
    fn scaled_buckets_add_up_to_the_target_and_keep_their_order() {
        let buckets: BTreeMap<String, u64> = [("a", 333), ("b", 333), ("c", 334), ("d", 1)]
            .map(|(k, v)| (k.to_string(), v))
            .into();
        let scaled = scale_to(&buckets, 500);
        assert_eq!(scaled.values().sum::<u64>(), 500);
        assert!(
            scaled["c"] >= scaled["a"] && scaled["a"] >= scaled["d"],
            "{scaled:?}"
        );
        // Proportional share: 334 / 1001 of 500 is 166.83.
        assert!((166..=167).contains(&scaled["c"]), "{scaled:?}");
        let empty: BTreeMap<String, u64> = [("a".to_string(), 0)].into();
        assert_eq!(scale_to(&empty, 10)["a"], 0);
    }
}

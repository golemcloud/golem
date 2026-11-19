use anyhow::{Context, bail, ensure};
use wasmparser::{Validator, WasmFeatures};

use crate::cores::{preserve_component_types, rewrite_cores};
use crate::sections::{BinaryKind, binary_kind};

/// Validates a core module or component with every proposal enabled, as
/// `wasm-tools validate --features all` does.
pub fn validate(wasm: &[u8]) -> anyhow::Result<()> {
    Validator::new_with_features(WasmFeatures::all())
        .validate_all(wasm)
        .context("the WebAssembly binary does not validate")?;
    Ok(())
}

/// The WIT text of a component's world.
pub fn world_wit(component: &[u8]) -> anyhow::Result<String> {
    let decoded =
        wit_parser::decoding::decode(component).context("could not decode the component's WIT")?;
    let mut printer = wit_component::WitPrinter::default();
    printer.print(decoded.resolve(), decoded.package(), &[])?;
    Ok(printer.output.to_string())
}

/// Replaces every core module of `wasm` with `optimize(core)` and keeps the rest of the
/// component byte for byte. The `component-type*` sections of each core are restored after the
/// optimizer, the result must validate, and a component must decode to the same WIT.
pub fn optimize_cores(
    wasm: &[u8],
    optimize: &mut dyn FnMut(&[u8]) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let output = rewrite_cores(wasm, &mut |core| {
        preserve_component_types(core, &optimize(core)?)
    })?;
    validate(&output)?;
    if binary_kind(wasm)? == BinaryKind::Component {
        ensure!(
            world_wit(wasm)? == world_wit(&output)?,
            "optimization changed the component's WIT contract"
        );
    }
    Ok(output)
}

/// The Binaryen flags of the per-core comparison. Enabling every proposal lets Binaryen
/// introduce typed references that Golem's engine rejects, so only the Rust `wasm32` baseline
/// features are enabled.
pub const WASM_OPT_ARGS: [&str; 8] = [
    "-Oz",
    "--enable-bulk-memory",
    "--enable-sign-ext",
    "--enable-nontrapping-float-to-int",
    "--enable-mutable-globals",
    "--enable-multivalue",
    "--enable-reference-types",
    "-g",
];

/// Runs `wasm-opt` on one core module. Only the explicitly invoked report calls this.
pub fn wasm_opt(core: &[u8], work: &std::path::Path, index: usize) -> anyhow::Result<Vec<u8>> {
    let source = work.join(format!("core-{index}.wasm"));
    let target = work.join(format!("core-{index}-Oz.wasm"));
    std::fs::write(&source, core)?;
    let status = std::process::Command::new("wasm-opt")
        .arg(&source)
        .args(WASM_OPT_ARGS)
        .arg("-o")
        .arg(&target)
        .status()
        .context("could not run wasm-opt")?;
    if !status.success() {
        bail!("wasm-opt failed on core {index}: {status}");
    }
    Ok(std::fs::read(target)?)
}

/// Runs `wasm-opt` on every core module of `wasm` with [`optimize_cores`], keeping the inputs
/// and outputs of each core in `work`.
pub fn optimize_with_wasm_opt(wasm: &[u8], work: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    std::fs::create_dir_all(work)?;
    let mut index = 0;
    optimize_cores(wasm, &mut |core| {
        index += 1;
        wasm_opt(core, work, index - 1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::CORE_HEADER;
    use test_r::test;

    fn component() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-components/concurrent-runtime-events/concurrent_runtime_events.wasm"
        ))
        .unwrap()
    }

    #[test]
    fn identity_core_replacement_keeps_the_wit_and_validates() {
        let original = component();
        let mut seen = 0;
        let output = optimize_cores(&original, &mut |core| {
            seen += 1;
            Ok(core.to_vec())
        })
        .unwrap();
        assert!(seen > 1);
        assert_eq!(output, original);
        assert!(world_wit(&output).unwrap().contains("world"));
    }

    #[test]
    fn a_core_replacement_that_breaks_the_component_fails() {
        let error = optimize_cores(&component(), &mut |_| Ok(CORE_HEADER.to_vec())).unwrap_err();
        assert!(
            format!("{error:#}").contains("does not validate"),
            "{error:#}"
        );
    }
}

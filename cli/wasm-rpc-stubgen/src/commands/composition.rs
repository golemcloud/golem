use crate::commands::log::log_action;
use anyhow::{anyhow, Context};
use golem_wasm_ast::analysis::{AnalysedExport, AnalysisContext, AnalysisFailure};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use std::fs;
use std::path::{Path, PathBuf};
use wasm_compose::config::Dependency;

pub fn compose(source_wasm: &Path, stub_wasm: &[PathBuf], dest_wasm: &Path) -> anyhow::Result<()> {
    let mut config = wasm_compose::config::Config::default();

    for stub_wasm in stub_wasm {
        let stub_bytes = fs::read(stub_wasm)?;
        let stub_component = Component::<IgnoreAllButMetadata>::from_bytes(&stub_bytes)
            .map_err(|err| anyhow!(err))?;

        let state = AnalysisContext::new(stub_component);
        let stub_exports = state.get_top_level_exports().map_err(|err| {
            let AnalysisFailure { reason } = err;
            anyhow!(reason)
        })?;

        for export in stub_exports {
            if let AnalysedExport::Instance(instance) = export {
                config.dependencies.insert(
                    instance.name.clone(),
                    Dependency {
                        path: stub_wasm.clone(),
                    },
                );
            }
        }
    }

    let composer = wasm_compose::composer::ComponentComposer::new(source_wasm, &config);
    let result = composer.compose()?;
    log_action("Writing", format!("composed component to {:?}", dest_wasm));
    fs::write(dest_wasm, result).context("Failed to write the composed component")?;
    Ok(())
}

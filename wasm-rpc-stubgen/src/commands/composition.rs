use crate::commands::log::log_warn_action;
use anyhow::Context;
use std::path::{Path, PathBuf};
use wac_graph::types::{Package, SubtypeChecker};
use wac_graph::{CompositionGraph, EncodeOptions, PackageId, PlugError};

pub async fn compose(
    source_wasm: &Path,
    stub_wasms: &[PathBuf],
    dest_wasm: &Path,
) -> anyhow::Result<()> {
    // Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/src/commands/plug.rs
    // with allowing missing plugs (through the also customized plug function below)
    // and using local packages only (for now)

    let mut graph = CompositionGraph::new();

    let socket = std::fs::read(source_wasm).with_context(|| {
        format!(
            "failed to read socket component `{socket}`",
            socket = source_wasm.to_string_lossy()
        )
    })?;

    let socket = Package::from_bytes("socket", None, socket, graph.types_mut())?;
    let socket = graph.register_package(socket)?;

    let mut plug_packages = Vec::new();
    for stub_wasm in stub_wasms {
        let plug_package = Package::from_file(
            &stub_wasm.to_string_lossy(),
            None,
            stub_wasm,
            graph.types_mut(),
        )?;
        let package_id = graph.register_package(plug_package)?;
        plug_packages.push(package_id);
    }

    plug(&mut graph, plug_packages, socket)?;

    let bytes = graph.encode(EncodeOptions::default())?;

    std::fs::write(dest_wasm, bytes).context(format!(
        "failed to write output file `{path}`",
        path = dest_wasm.display()
    ))?;

    Ok(())
}

// Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/crates/wac-graph/src/plug.rs#L23
// but instead of returning NoPlugError, it logs skipped instantiations
fn plug(
    graph: &mut CompositionGraph,
    plugs: Vec<PackageId>,
    socket: PackageId,
) -> Result<(), PlugError> {
    let socket_instantiation = graph.instantiate(socket);

    for plug in plugs {
        let mut plug_exports = Vec::new();
        let mut cache = Default::default();
        let mut checker = SubtypeChecker::new(&mut cache);
        for (name, plug_ty) in &graph.types()[graph[plug].ty()].exports {
            if let Some(socket_ty) = graph.types()[graph[socket].ty()].imports.get(name) {
                if checker
                    .is_subtype(*plug_ty, graph.types(), *socket_ty, graph.types())
                    .is_ok()
                {
                    plug_exports.push(name.clone());
                }
            }
        }

        // Instantiate the plug component
        let mut plug_instantiation = None;
        for plug_name in plug_exports {
            let plug_instantiation =
                *plug_instantiation.get_or_insert_with(|| graph.instantiate(plug));
            let export = graph
                .alias_instance_export(plug_instantiation, &plug_name)
                .map_err(|err| PlugError::GraphError { source: err.into() })?;
            graph
                .set_instantiation_argument(socket_instantiation, &plug_name, export)
                .map_err(|err| PlugError::GraphError { source: err.into() })?;
        }
    }

    for (name, _) in graph.get_instantiation_arguments(socket_instantiation) {
        log_warn_action("Skipping", format!("instantiation of {}, not used", name));
    }

    // Export all exports from the socket component.
    for name in graph.types()[graph[socket].ty()]
        .exports
        .keys()
        .cloned()
        .collect::<Vec<_>>()
    {
        let export = graph
            .alias_instance_export(socket_instantiation, &name)
            .map_err(|err| PlugError::GraphError { source: err.into() })?;

        graph
            .export(export, &name)
            .map_err(|err| PlugError::GraphError { source: err.into() })?;
    }

    Ok(())
}

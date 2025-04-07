use crate::fs;
use crate::fs::PathExtra;
use crate::log::{log_warn_action, LogColorize};
use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};
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

    let dest_wasm = PathExtra::new(dest_wasm);

    let mut graph = CompositionGraph::new();

    let socket = fs::read(source_wasm).context("Failed to read socket component")?;

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
        plug_packages.push((stub_wasm.to_string_lossy().to_string(), package_id));
    }

    plug(&mut graph, plug_packages, socket)?;

    let bytes = graph.encode(EncodeOptions::default())?;

    fs::create_dir_all(dest_wasm.parent()?)?;
    fs::write(dest_wasm, bytes)?;

    Ok(())
}

// Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/crates/wac-graph/src/plug.rs#L23
// but instead of returning NoPlugError, it logs skipped instantiations
fn plug(
    graph: &mut CompositionGraph,
    plugs: Vec<(String, PackageId)>,
    socket: PackageId,
) -> Result<(), PlugError> {
    let socket_instantiation = graph.instantiate(socket);

    let mut requested_plugs = BTreeSet::<String>::new();
    let mut plug_exports_to_plug = BTreeMap::<String, String>::new();

    for (plug_name, plug) in plugs {
        requested_plugs.insert(plug_name.clone());

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
        for plug_export_name in plug_exports {
            plug_exports_to_plug.insert(plug_export_name.clone(), plug_name.clone());

            let plug_instantiation =
                *plug_instantiation.get_or_insert_with(|| graph.instantiate(plug));
            let export = graph
                .alias_instance_export(plug_instantiation, &plug_export_name)
                .map_err(|err| PlugError::GraphError { source: err.into() })?;
            graph
                .set_instantiation_argument(socket_instantiation, &plug_export_name, export)
                .map_err(|err| PlugError::GraphError { source: err.into() })?;
        }
    }

    let unused_plugs = {
        for (plug_export_name, _) in graph.get_instantiation_arguments(socket_instantiation) {
            plug_exports_to_plug
                .remove(plug_export_name)
                .iter()
                .for_each(|plug_name| {
                    requested_plugs.remove(plug_name);
                });
        }
        requested_plugs
    };

    for plug_name in unused_plugs {
        log_warn_action(
            "Skipping",
            format!("{}, not used", plug_name.log_color_highlight()),
        );
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

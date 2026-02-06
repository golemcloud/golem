use crate::fs;
use anyhow::{anyhow, Context};
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use wac_graph::types::{Package, SubtypeChecker};
use wac_graph::{CompositionGraph, EncodeOptions, NodeId, PackageId, PlugError};

#[derive(Debug, Clone)]
pub struct Plug {
    pub name: String,
    pub wasm: PathBuf,
}

pub async fn compose(
    source_wasm: &Path,
    plugs: Vec<Plug>,
    dest_wasm: &Path,
) -> anyhow::Result<Vec<String>> {
    // Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/src/commands/plug.rs
    // with allowing missing plugs (through the also customized plug function below)
    // and using local packages only (for now)

    let mut graph = CompositionGraph::new();

    let socket = fs::read(source_wasm).context("Failed to read socket component")?;

    let socket = Package::from_bytes("socket", None, socket, graph.types_mut())?;
    let socket = graph.register_package(socket)?;

    let mut plug_packages = Vec::new();
    for plug in plugs {
        let plug_package = Package::from_file(&plug.name, None, &plug.wasm, graph.types_mut())?;
        let package_id = graph.register_package(plug_package)?;
        plug_packages.push((plug, package_id));
    }

    let unused_plugs = plug(&mut graph, plug_packages, socket)?;

    let bytes = graph.encode(EncodeOptions::default())?;

    fs::create_dir_all(fs::parent_or_err(dest_wasm)?)?;
    fs::write(dest_wasm, bytes)?;

    Ok(unused_plugs)
}

// Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/crates/wac-graph/src/plug.rs#L23,
// but:
//   - instead of returning NoPlugError, it returns skipped instantiations
//   - pre-checks multi plugs for the same export to prevent ArgumentAlreadyPassed errors, and
//     returns a custom error for it with more context
fn plug(
    graph: &mut CompositionGraph,
    plugs: Vec<(Plug, PackageId)>,
    socket: PackageId,
) -> Result<Vec<String>, PlugError> {
    let socket_instantiation = graph.instantiate(socket);

    let mut offered_plugs = BTreeSet::<&str>::new();
    let mut plug_exports_to_plug = BTreeMap::<String, Vec<(&Plug, PackageId)>>::new();

    for (plug, plug_package_id) in &plugs {
        offered_plugs.insert(&plug.name);
        let mut cache = Default::default();
        let mut checker = SubtypeChecker::new(&mut cache);
        for (name, plug_ty) in &graph.types()[graph[*plug_package_id].ty()].exports {
            if let Some(socket_ty) = graph.types()[graph[socket].ty()].imports.get(name) {
                if checker
                    .is_subtype(*plug_ty, graph.types(), *socket_ty, graph.types())
                    .is_ok()
                {
                    plug_exports_to_plug
                        .entry(name.clone())
                        .or_default()
                        .push((plug, *plug_package_id));
                }
            }
        }
    }

    // Instantiate plugs
    let mut plug_instantiations = BTreeMap::<PackageId, Option<NodeId>>::new();
    for (plug_export_name, plugs) in plug_exports_to_plug.iter() {
        if plugs.len() > 1 {
            return Err(PlugError::GraphError {
                source: anyhow!(
                    "multiple plugs found for export {}, only use one of them:\n{}\n",
                    plug_export_name,
                    plugs
                        .iter()
                        .map(|p| format!("  - {}", &p.0.name))
                        .join("\n")
                ),
            });
        }
        let (_plug, plug_package_id) = plugs.first().unwrap();

        let plug_instantiation = *plug_instantiations
            .entry(*plug_package_id)
            .or_default()
            .get_or_insert_with(|| graph.instantiate(*plug_package_id));
        let export = graph
            .alias_instance_export(plug_instantiation, plug_export_name)
            .map_err(|err| PlugError::GraphError { source: err.into() })?;
        graph
            .set_instantiation_argument(socket_instantiation, plug_export_name, export)
            .map_err(|err| PlugError::GraphError { source: err.into() })?;
    }

    let unused_plugs = {
        for (plug_export_name, _) in graph.get_instantiation_arguments(socket_instantiation) {
            plug_exports_to_plug
                .remove(plug_export_name)
                .iter()
                .flat_map(|plugs| plugs.iter().map(|(plug, _)| plug.name.as_str()))
                .for_each(|plug_name| {
                    offered_plugs.remove(plug_name);
                });
        }
        offered_plugs
    };

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

    Ok(unused_plugs
        .into_iter()
        .map(|plug_name| plug_name.to_string())
        .collect())
}

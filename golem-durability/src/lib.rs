// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use wac_graph::types::{Package, SubtypeChecker};
use wac_graph::{CompositionGraph, EncodeOptions, PackageId, PlugError};

pub const DURABLE_WASI_WASM: &'static [u8] = include_bytes!(env!("DURABLE_WASI_COMPONENT"));

pub fn compose_with_durability_in_fs(source_wasm: &Path, target_wasm: &Path) -> anyhow::Result<()> {
    // Based on https://github.com/bytecodealliance/wac/blob/release-0.6.0/src/commands/plug.rs
    // with allowing missing plugs (through the also customized plug function below)
    // and using local packages only (for now)

    let mut graph = CompositionGraph::new();

    let socket = std::fs::read(source_wasm).context("Failed to read socket component")?;

    let socket = Package::from_bytes("socket", None, socket, graph.types_mut())?;
    let socket = graph.register_package(socket)?;

    let mut plug_packages = Vec::new();
    let plug_package =
        Package::from_bytes("durable_wasi", None, DURABLE_WASI_WASM, graph.types_mut())?;
    let package_id = graph.register_package(plug_package)?;
    plug_packages.push(("durable_wasi".to_string(), package_id));

    plug(&mut graph, plug_packages, socket)?;

    let bytes = graph.encode(EncodeOptions::default())?;

    std::fs::write(target_wasm, bytes)?;

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
        log::warn!("Skipping {}, not used", plug_name);
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

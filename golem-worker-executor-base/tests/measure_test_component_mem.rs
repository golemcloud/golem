use crate::common::{start, TestContext, TestWorkerExecutor};
use anyhow::anyhow;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_ast::analysis::AnalysisContext;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use humansize::{ISizeFormatter, BINARY};
use rand::prelude::SliceRandom;
use rand::thread_rng;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use sysinfo::{Pid, System};
use tracing::{error, info};

#[tokio::test]
#[ignore]
async fn measure() {
    let mut system = System::new_all();
    let ctx = TestContext::new();
    let executor = start(&ctx).await.unwrap();

    // collect
    let mut paths = Vec::new();
    let mut read_dir = tokio::fs::read_dir(executor.component_directory())
        .await
        .unwrap();
    while let Some(entry) = read_dir.next_entry().await.unwrap() {
        if entry.file_name().to_string_lossy().ends_with(".wasm") {
            paths.push(entry.path());
        }
    }

    let mut rng = thread_rng();

    // measure
    let mut results = BTreeMap::new();
    for _idx in 0..3 {
        paths.shuffle(&mut rng);
        for path in &paths {
            let component_size = tokio::fs::metadata(path).await.unwrap().len();
            if let Ok(result) = measure_component(&mut system, &executor, path).await {
                let entry: &mut Vec<(u64, (i64, i64))> = results
                    .entry(path.file_name().unwrap().to_string_lossy().to_string())
                    .or_default();
                entry.push((component_size, result));
            } else {
                error!("Failed to measure {:?}", path);
            }
        }
    }

    drop(executor);

    let mut csv = String::new();
    for (name, inner_results) in results {
        let mut component_size = 0;
        let mut delta_memory = 0;
        let mut vdelta_memory = 0;

        for (size, (d, vd)) in &inner_results {
            component_size += *size;
            delta_memory += *d;
            vdelta_memory += *vd;
        }

        let component_size = component_size / inner_results.len() as u64;
        let delta_memory = delta_memory / inner_results.len() as i64;
        let _vdelta_memory = vdelta_memory / inner_results.len() as i64;

        info!(
            "{}: component size: {}, avg delta memory: {}",
            name,
            ISizeFormatter::new(component_size, BINARY),
            ISizeFormatter::new(delta_memory, BINARY),
            // ISizeFormatter::new(vresult, BINARY)
        );
        writeln!(csv, "{},{},{}", name, component_size, delta_memory).unwrap();
    }
    info!("{}", csv);
}

async fn measure_component(
    system: &mut System,
    executor: &TestWorkerExecutor,
    path: &Path,
) -> anyhow::Result<(i64, i64)> {
    info!("Measuring {path:?}");

    let component_id = executor
        .component_service()
        .get_or_add_component(path)
        .await;

    let data = std::fs::read(path)?;
    let component =
        Component::<IgnoreAllButMetadata>::from_bytes(&data).map_err(|err| anyhow!(err))?;
    let state = AnalysisContext::new(component);
    let mems = state
        .get_all_memories()
        .map_err(|err| anyhow!(format!("{:?}", err)))?;

    let mut results = Vec::new();

    for _idx in 0..2 {
        let pid = Pid::from_u32(std::process::id());
        system.refresh_process(pid);
        let process = system.process(pid).unwrap();
        let before_memory = process.memory();
        let before_vmemory = process.virtual_memory();

        let worker_id = executor.start_worker(&component_id, "measure").await;

        system.refresh_process(pid);
        let process = system.process(pid).unwrap();
        let after_memory = process.memory();
        let after_vmemory = process.virtual_memory();

        executor.delete_worker(&worker_id).await;

        // Not substracting total_initial_mem because it is only allocated runtime
        let delta_memory = after_memory as i64 - before_memory as i64;
        let delta_vmemory = after_vmemory as i64 - before_vmemory as i64;

        info!(
            "{:?} memory: {} -> {} ({:?})",
            path, before_memory, after_memory, delta_memory
        );

        info!(
            "{:?} virtual memory: {} -> {} ({:?})",
            path, before_vmemory, after_vmemory, delta_vmemory
        );
        let total_initial_mem = mems
            .iter()
            .map(|mem| mem.mem_type.limits.min * 65536)
            .sum::<u64>();
        info!("{:?} initial memory: {}", path, total_initial_mem);

        if delta_memory >= 0 {
            results.push((delta_memory, delta_vmemory));
        }
    }

    if !results.is_empty() {
        let delta_memory = results.iter().map(|(d, _)| d).sum::<i64>() / results.len() as i64;
        let delta_vmemory = results.iter().map(|(_, d)| d).sum::<i64>() / results.len() as i64;
        Ok((delta_memory, delta_vmemory))
    } else {
        Err(anyhow!("No results"))
    }
}

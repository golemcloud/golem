use crate::common::{start, TestContext, TestWorkerExecutor};
use anyhow::anyhow;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_ast::analysis::AnalysisContext;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use humansize::{ISizeFormatter, BINARY};
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

    // measure
    let mut read_dir = tokio::fs::read_dir(executor.component_directory())
        .await
        .unwrap();
    let mut results = BTreeMap::new();
    while let Some(entry) = read_dir.next_entry().await.unwrap() {
        if entry.file_name().to_string_lossy().ends_with(".wasm") {
            let component_size = tokio::fs::metadata(entry.path()).await.unwrap().len();
            if let Ok(result) = measure_component(&mut system, &executor, &entry.path()).await {
                results.insert(
                    entry.file_name().to_string_lossy().to_string(),
                    (component_size, result),
                );
            } else {
                error!("Failed to measure {:?}", entry.path());
            }
        }
    }

    drop(executor);

    let mut csv = String::new();
    for (name, (component_size, (result, _vresult))) in results {
        info!(
            "{}: component size: {}, avg delta memory: {}",
            name,
            ISizeFormatter::new(component_size, BINARY),
            ISizeFormatter::new(result, BINARY),
            // ISizeFormatter::new(vresult, BINARY)
        );
        writeln!(csv, "{},{},{}", name, component_size, result).unwrap();
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
        .get_or_add_component(&path)
        .await;

    let data = std::fs::read(path)?;
    let component =
        Component::<IgnoreAllButMetadata>::from_bytes(&data).map_err(|err| anyhow!(err))?;
    let state = AnalysisContext::new(component);
    let mems = state
        .get_all_memories()
        .map_err(|err| anyhow!(format!("{:?}", err)))?;

    let mut results = Vec::new();

    for idx in 0..6 {
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

        // first try is warmup
        if idx > 0 && delta_memory > 0 {
            results.push((delta_memory, delta_vmemory));
        }
    }

    if results.len() > 0 {
        let delta_memory = results.iter().map(|(d, _)| d).sum::<i64>() / results.len() as i64;
        let delta_vmemory = results.iter().map(|(_, d)| d).sum::<i64>() / results.len() as i64;
        Ok((delta_memory, delta_vmemory))
    } else {
        Err(anyhow!("No results"))
    }
}

//! Use the same component task runtime as the SDK's tool imports.
mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "shell-services", generate_all });
}

pub fn services() -> bash_shell::ExecutionServices {
    bash_shell::ExecutionServices {
        spawn_local: |future| {
            golem_rust::agentic::spawn_local(future);
        },
        yield_now: || Box::pin(wit_bindgen::yield_async()),
        sleep: |duration| {
            Box::pin(async move {
                let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
                bindings::wasi::clocks::monotonic_clock::wait_for(nanos).await;
            })
        },
    }
}

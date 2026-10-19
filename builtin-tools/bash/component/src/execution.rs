//! Use the same component task runtime as the SDK's tool imports.
mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "shell-services", generate_all });
}

thread_local! {
    static YIELDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub fn services() -> bash_shell::ExecutionServices {
    bash_shell::ExecutionServices {
        spawn_local: |future| {
            golem_rust::agentic::spawn_local(future);
        },
        yield_now: || {
            Box::pin(async {
                let block = YIELDS.with(|yields| {
                    let next = (yields.get() + 1) % 1024;
                    yields.set(next);
                    next == 0
                });
                if block {
                    // Every script has a time limit; async stop/timer events need blocked
                    // waits. Amortize the durable Start, End and CompletionDelivered cost
                    // over 1024 yields rather than recording a call on every yield.
                    bindings::wasi::clocks::monotonic_clock::wait_for(0).await;
                } else {
                    wit_bindgen::yield_async().await;
                }
            })
        },
        sleep: |duration| {
            Box::pin(async move {
                let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
                bindings::wasi::clocks::monotonic_clock::wait_for(nanos).await;
            })
        },
    }
}

//! Use the same component task runtime as the SDK's tool imports.
mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "shell-services", generate_all });
}

thread_local! {
    static WATCHES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static YIELDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub struct Watch;

impl Watch {
    pub fn arm() -> Self {
        WATCHES.with(|watches| watches.set(watches.get() + 1));
        Self
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        WATCHES.with(|watches| watches.set(watches.get() - 1));
    }
}

pub fn services() -> bash_shell::ExecutionServices {
    bash_shell::ExecutionServices {
        spawn_local: |future| {
            golem_rust::agentic::spawn_local(future);
        },
        yield_now: || {
            Box::pin(async {
                let block = WATCHES.with(|watches| watches.get() != 0)
                    && YIELDS.with(|yields| {
                        let next = (yields.get() + 1) % 1024;
                        yields.set(next);
                        next == 0
                    });
                if block {
                    // Async stop/timer events arrive only at blocked waits. Amortize the
                    // durable Start, End and CompletionDelivered cost over 1024 yields.
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

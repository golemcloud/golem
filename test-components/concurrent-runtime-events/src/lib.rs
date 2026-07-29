mod bindings {
    // `async` is intentionally not set so the WIT source annotations decide:
    // `call` (and the two exports) are `async func`, while `observed` is a plain
    // synchronous `func`.
    wit_bindgen::generate!({
        path: "wit",
        world: "runtime-events",
    });

    use super::Component;
    export!(Component);
}

use bindings::golem::cmtest::host;
use futures::stream::{FuturesUnordered, StreamExt};

struct Component;

impl bindings::Guest for Component {
    async fn cancel_one() -> Vec<u32> {
        let mut pending = FuturesUnordered::new();
        for id in 0..2 {
            pending.push(async move {
                let value = host::call(id).await;
                (id, value)
            });
        }

        let (winner_id, _winner_value) = pending.next().await.expect("one completion");

        // Dropping the remaining in-flight call cancels it: `wit-bindgen` issues
        // `subtask.cancel` synchronously from the future's destructor, which the
        // runtime turns into aborting/dropping the host future.
        drop(pending);

        vec![winner_id]
    }

    async fn observe_completions(count: u32) -> Vec<u32> {
        let mut pending = FuturesUnordered::new();
        for id in 0..count {
            pending.push(async move {
                let value = host::call(id).await;
                (id, value)
            });
        }

        let mut order = Vec::new();
        while let Some((id, value)) = pending.next().await {
            // Notify the host the moment we observe this completion, so the host
            // can assert its `call` body fully ran before we were made ready.
            host::observed(id, value);
            order.push(id);
        }
        order
    }
}

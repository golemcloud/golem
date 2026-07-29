mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "delivery-order",
        async: true,
    });

    use super::Component;
    export!(Component);
}

use bindings::golem::cmtest::host;
use futures::stream::{FuturesUnordered, StreamExt};

struct Component;

impl bindings::Guest for Component {
    async fn run(count: u32) -> Vec<u32> {
        let mut pending = FuturesUnordered::new();
        for id in 0..count {
            pending.push(async move {
                host::call(id).await;
                id
            });
        }
        let mut order = Vec::new();
        while let Some(id) = pending.next().await {
            order.push(id);
        }
        order
    }
}

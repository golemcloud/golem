// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::{OpenApiDocument, OpenApiError, OpenApiInputs, OpenApiService};
use crate::metrics::{record_openapi_cache, record_openapi_generation};
use golem_common::cache::SimpleCache;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::Instrument;

#[cfg(test)]
mod tests;

type Result = std::result::Result<Arc<OpenApiDocument>, OpenApiError>;

impl OpenApiService {
    pub async fn generate(&self, inputs: Arc<OpenApiInputs>) -> Result {
        let key = inputs.key.clone();
        let service = self.clone();
        self.cache
            .get_or_insert_simple_spawned(&key, move || async move {
                record_openapi_cache("miss");
                let started = Instant::now();
                let result = service
                    .run(inputs)
                    .instrument(tracing::info_span!("generate_openapi"))
                    .await;
                let outcome = result
                    .as_ref()
                    .map_or_else(|error| error.category(), |_| "success");
                record_openapi_generation(outcome, started.elapsed());
                tracing::debug!(outcome, "OpenAPI generation completed");
                result
            })
            .await
    }
}

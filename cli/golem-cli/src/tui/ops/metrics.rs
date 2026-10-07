// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::local_observability::LocalObservabilityClient;
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::local_observability::{
    LocalMetricFamiliesRequest, LocalMetricFamily, LocalMetricSample, LocalMetricSamplesRequest,
    LocalMetricScope, LocalMetricSeries, LocalMetricSeriesRequest, LocalMetricSource,
    LocalMetricTimeRange, LocalObservabilityCapability, LocalObservabilityStatus,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(in crate::tui) const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_RANGE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum MetricsFocus {
    Families,
    Series,
    Chart,
}

impl MetricsFocus {
    pub(in crate::tui) fn next(self) -> Self {
        match self {
            Self::Families => Self::Series,
            Self::Series => Self::Chart,
            Self::Chart => Self::Families,
        }
    }

    pub(in crate::tui) fn previous(self) -> Self {
        match self {
            Self::Families => Self::Chart,
            Self::Series => Self::Families,
            Self::Chart => Self::Series,
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct MetricsQuery {
    pub(in crate::tui) source: LocalMetricSource,
    pub(in crate::tui) agent_ids: Vec<String>,
    pub(in crate::tui) selected_family: Option<String>,
    pub(in crate::tui) selected_series: Option<String>,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct MetricsRefreshPayload {
    pub(in crate::tui) capability: LocalObservabilityCapability,
    pub(in crate::tui) source: LocalMetricSource,
    pub(in crate::tui) families: Vec<LocalMetricFamily>,
    pub(in crate::tui) series: Vec<LocalMetricSeries>,
    pub(in crate::tui) samples: Vec<LocalMetricSample>,
    pub(in crate::tui) selected_family: usize,
    pub(in crate::tui) selected_series: usize,
}

pub(in crate::tui) struct MetricsState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) capability: Option<LocalObservabilityCapability>,
    pub(in crate::tui) source: LocalMetricSource,
    pub(in crate::tui) families: Vec<LocalMetricFamily>,
    pub(in crate::tui) series: Vec<LocalMetricSeries>,
    pub(in crate::tui) samples: Vec<LocalMetricSample>,
    pub(in crate::tui) selected_family: usize,
    pub(in crate::tui) selected_series: usize,
    pub(in crate::tui) focus: MetricsFocus,
    pub(in crate::tui) error: Option<String>,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            capability: None,
            source: LocalMetricSource::Agents,
            families: Vec::new(),
            series: Vec::new(),
            samples: Vec::new(),
            selected_family: 0,
            selected_series: 0,
            focus: MetricsFocus::Families,
            error: None,
        }
    }
}

impl MetricsState {
    pub(in crate::tui) fn selected_family_name(&self) -> Option<String> {
        self.families
            .get(self.selected_family)
            .map(|family| family.name.clone())
    }

    pub(in crate::tui) fn selected_series_key(&self) -> Option<String> {
        self.series
            .get(self.selected_series)
            .map(|series| series.series_key.clone())
    }
}

pub(in crate::tui) async fn load(
    context: &Arc<Context>,
    environment_reference: Option<&EnvironmentReference>,
    query: MetricsQuery,
) -> anyhow::Result<MetricsRefreshPayload> {
    let client = LocalObservabilityClient::new(
        context.file_download_client().clone(),
        context.worker_service_url().clone(),
    );
    let capability = client.capability().await?.ok_or_else(|| {
        anyhow::anyhow!("The selected server does not provide local observability")
    })?;
    if capability.status != LocalObservabilityStatus::Ready {
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            families: Vec::new(),
            series: Vec::new(),
            samples: Vec::new(),
            selected_family: 0,
            selected_series: 0,
        });
    }

    let mut scope = LocalMetricScope {
        agent_ids: query.agent_ids,
        ..Default::default()
    };
    if query.source == LocalMetricSource::Agents {
        let environment = context
            .environment_handler()
            .resolve_opt_environment_reference_read_only(
                EnvironmentResolveMode::Any,
                environment_reference,
            )
            .await?;
        scope.environment_id = Some(environment.environment_id.0);
    }
    let range = current_range();
    let families = client
        .families(&LocalMetricFamiliesRequest {
            source: query.source,
            scope: scope.clone(),
            range,
            name_filter: None,
            cursor: None,
            limit: None,
        })
        .await?
        .values;
    let selected_family = query
        .selected_family
        .as_ref()
        .and_then(|selected| families.iter().position(|family| family.name == *selected))
        .unwrap_or(0)
        .min(families.len().saturating_sub(1));
    let Some(family) = families.get(selected_family) else {
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            families,
            series: Vec::new(),
            samples: Vec::new(),
            selected_family: 0,
            selected_series: 0,
        });
    };
    let series = client
        .series(&LocalMetricSeriesRequest {
            source: query.source,
            scope: scope.clone(),
            range,
            family: family.name.clone(),
            cursor: None,
            limit: None,
        })
        .await?
        .values;
    let selected_series = query
        .selected_series
        .as_ref()
        .and_then(|selected| {
            series
                .iter()
                .position(|series| series.series_key == *selected)
        })
        .unwrap_or(0)
        .min(series.len().saturating_sub(1));
    let samples = match series.get(selected_series) {
        Some(series) => {
            client
                .samples(&LocalMetricSamplesRequest {
                    source: query.source,
                    scope,
                    range,
                    family: family.name.clone(),
                    series_key: series.series_key.clone(),
                    max_points: None,
                })
                .await?
                .samples
        }
        None => Vec::new(),
    };

    Ok(MetricsRefreshPayload {
        capability,
        source: query.source,
        families,
        series,
        samples,
        selected_family,
        selected_series,
    })
}

fn current_range() -> LocalMetricTimeRange {
    let to = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    LocalMetricTimeRange {
        from_unix_nanos: to.saturating_sub(DEFAULT_RANGE.as_nanos() as i64),
        to_unix_nanos: to,
    }
}

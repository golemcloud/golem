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
    LocalMetricDashboardRequest, LocalMetricDashboardResponse, LocalMetricFamiliesRequest,
    LocalMetricFamily, LocalMetricSample, LocalMetricSamplesRequest, LocalMetricSamplesResponse,
    LocalMetricScope, LocalMetricSeries, LocalMetricSeriesRequest, LocalMetricSource,
    LocalMetricTimeRange, LocalObservabilityCapability, LocalObservabilityStatus,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(in crate::tui) const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
pub(in crate::tui) const MAX_EXPLORE_SERIES: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum MetricsLoad {
    Replace,
    AppendFamilies,
    SelectFamily,
    AppendSeries,
    Samples,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum MetricsMode {
    Dashboard,
    Explore,
}

impl MetricsMode {
    pub(in crate::tui) fn previous(self) -> Self {
        match self {
            Self::Dashboard => Self::Explore,
            Self::Explore => Self::Dashboard,
        }
    }

    pub(in crate::tui) fn next(self) -> Self {
        self.previous()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum MetricsRange {
    Minutes5,
    Minutes15,
    Hour1,
    Hours6,
    Hours24,
    Days7,
}

impl MetricsRange {
    pub(in crate::tui) const fn label(self) -> &'static str {
        match self {
            Self::Minutes5 => "5m",
            Self::Minutes15 => "15m",
            Self::Hour1 => "1h",
            Self::Hours6 => "6h",
            Self::Hours24 => "24h",
            Self::Days7 => "7d",
        }
    }

    const fn duration(self) -> Duration {
        match self {
            Self::Minutes5 => Duration::from_secs(5 * 60),
            Self::Minutes15 => Duration::from_secs(15 * 60),
            Self::Hour1 => Duration::from_secs(60 * 60),
            Self::Hours6 => Duration::from_secs(6 * 60 * 60),
            Self::Hours24 => Duration::from_secs(24 * 60 * 60),
            Self::Days7 => Duration::from_secs(7 * 24 * 60 * 60),
        }
    }

    pub(in crate::tui) fn next(self) -> Self {
        match self {
            Self::Minutes5 => Self::Minutes15,
            Self::Minutes15 => Self::Hour1,
            Self::Hour1 => Self::Hours6,
            Self::Hours6 => Self::Hours24,
            Self::Hours24 => Self::Days7,
            Self::Days7 => Self::Minutes5,
        }
    }
}

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
    pub(in crate::tui) mode: MetricsMode,
    pub(in crate::tui) agent_ids: Vec<String>,
    pub(in crate::tui) selected_family: Option<String>,
    pub(in crate::tui) selected_series: Option<String>,
    pub(in crate::tui) selected_series_keys: Vec<String>,
    pub(in crate::tui) range: MetricsRange,
    pub(in crate::tui) load: MetricsLoad,
    pub(in crate::tui) families_cursor: Option<String>,
    pub(in crate::tui) series_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct MetricsSeriesSamples {
    pub(in crate::tui) series_key: String,
    pub(in crate::tui) response: LocalMetricSamplesResponse,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct MetricsRefreshPayload {
    pub(in crate::tui) capability: LocalObservabilityCapability,
    pub(in crate::tui) source: LocalMetricSource,
    pub(in crate::tui) mode: MetricsMode,
    pub(in crate::tui) load: MetricsLoad,
    pub(in crate::tui) time_range: LocalMetricTimeRange,
    pub(in crate::tui) dashboard: Option<LocalMetricDashboardResponse>,
    pub(in crate::tui) families: Vec<LocalMetricFamily>,
    pub(in crate::tui) series: Vec<LocalMetricSeries>,
    pub(in crate::tui) samples: Vec<LocalMetricSample>,
    pub(in crate::tui) series_samples: Vec<MetricsSeriesSamples>,
    pub(in crate::tui) selected_series_keys: Vec<String>,
    pub(in crate::tui) families_next_cursor: Option<String>,
    pub(in crate::tui) series_next_cursor: Option<String>,
    pub(in crate::tui) selected_family: usize,
    pub(in crate::tui) selected_series: usize,
}

pub(in crate::tui) struct MetricsState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) capability: Option<LocalObservabilityCapability>,
    pub(in crate::tui) source: LocalMetricSource,
    pub(in crate::tui) mode: MetricsMode,
    pub(in crate::tui) range: MetricsRange,
    pub(in crate::tui) live: bool,
    pub(in crate::tui) time_range: Option<LocalMetricTimeRange>,
    pub(in crate::tui) selected_panel: usize,
    pub(in crate::tui) dashboard: Option<LocalMetricDashboardResponse>,
    pub(in crate::tui) families: Vec<LocalMetricFamily>,
    pub(in crate::tui) series: Vec<LocalMetricSeries>,
    pub(in crate::tui) samples: Vec<LocalMetricSample>,
    pub(in crate::tui) series_samples: Vec<MetricsSeriesSamples>,
    pub(in crate::tui) selected_series_keys: Vec<String>,
    pub(in crate::tui) families_next_cursor: Option<String>,
    pub(in crate::tui) series_next_cursor: Option<String>,
    pub(in crate::tui) selected_family: usize,
    pub(in crate::tui) selected_series: usize,
    pub(in crate::tui) chart_cursor: Option<usize>,
    pub(in crate::tui) focus: MetricsFocus,
    pub(in crate::tui) error: Option<String>,
    pub(in crate::tui) notice: Option<String>,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            capability: None,
            source: LocalMetricSource::Agents,
            mode: MetricsMode::Dashboard,
            range: MetricsRange::Minutes15,
            live: true,
            time_range: None,
            selected_panel: 0,
            dashboard: None,
            families: Vec::new(),
            series: Vec::new(),
            samples: Vec::new(),
            series_samples: Vec::new(),
            selected_series_keys: Vec::new(),
            families_next_cursor: None,
            series_next_cursor: None,
            selected_family: 0,
            selected_series: 0,
            chart_cursor: None,
            focus: MetricsFocus::Families,
            error: None,
            notice: None,
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
    let range = current_range(query.range);
    if capability.status != LocalObservabilityStatus::Ready {
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            mode: query.mode,
            load: query.load,
            time_range: range,
            dashboard: None,
            families: Vec::new(),
            series: Vec::new(),
            samples: Vec::new(),
            series_samples: Vec::new(),
            selected_series_keys: Vec::new(),
            families_next_cursor: None,
            series_next_cursor: None,
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
    if query.mode == MetricsMode::Dashboard {
        let dashboard = client
            .dashboard(&LocalMetricDashboardRequest {
                source: query.source,
                scope,
                range,
                max_points: Some(80),
            })
            .await?;
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            mode: query.mode,
            load: query.load,
            time_range: range,
            dashboard: Some(dashboard),
            families: Vec::new(),
            series: Vec::new(),
            samples: Vec::new(),
            series_samples: Vec::new(),
            selected_series_keys: Vec::new(),
            families_next_cursor: None,
            series_next_cursor: None,
            selected_family: 0,
            selected_series: 0,
        });
    }
    let (families, families_next_cursor, selected_family, family_name) = match query.load {
        MetricsLoad::Replace | MetricsLoad::AppendFamilies => {
            let response = client
                .families(&LocalMetricFamiliesRequest {
                    source: query.source,
                    scope: scope.clone(),
                    range,
                    name_filter: None,
                    cursor: query.families_cursor.clone(),
                    limit: None,
                })
                .await?;
            if query.load == MetricsLoad::AppendFamilies {
                return Ok(MetricsRefreshPayload {
                    capability,
                    source: query.source,
                    mode: query.mode,
                    load: query.load,
                    time_range: range,
                    dashboard: None,
                    families: response.values,
                    series: Vec::new(),
                    samples: Vec::new(),
                    series_samples: Vec::new(),
                    selected_series_keys: Vec::new(),
                    families_next_cursor: response.next_cursor,
                    series_next_cursor: None,
                    selected_family: 0,
                    selected_series: 0,
                });
            }
            let families = response.values;
            let selected_family = query
                .selected_family
                .as_ref()
                .and_then(|selected| families.iter().position(|family| family.name == *selected))
                .unwrap_or(0)
                .min(families.len().saturating_sub(1));
            let family_name = families
                .get(selected_family)
                .map(|family| family.name.clone());
            (families, response.next_cursor, selected_family, family_name)
        }
        MetricsLoad::SelectFamily | MetricsLoad::AppendSeries | MetricsLoad::Samples => {
            (Vec::new(), None, 0, query.selected_family.clone())
        }
    };
    let Some(family_name) = family_name else {
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            mode: query.mode,
            load: query.load,
            time_range: range,
            dashboard: None,
            families,
            series: Vec::new(),
            samples: Vec::new(),
            series_samples: Vec::new(),
            selected_series_keys: Vec::new(),
            families_next_cursor,
            series_next_cursor: None,
            selected_family: 0,
            selected_series: 0,
        });
    };
    if query.load == MetricsLoad::Samples {
        let selected_series_keys = query
            .selected_series_keys
            .into_iter()
            .take(MAX_EXPLORE_SERIES)
            .collect::<Vec<_>>();
        let series_samples = load_series_samples(
            &client,
            query.source,
            &scope,
            range,
            &family_name,
            &selected_series_keys,
        )
        .await?;
        let samples = series_samples
            .iter()
            .find(|samples| query.selected_series.as_deref() == Some(&samples.series_key))
            .or_else(|| series_samples.first())
            .map(|samples| samples.response.samples.clone())
            .unwrap_or_default();
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            mode: query.mode,
            load: query.load,
            time_range: range,
            dashboard: None,
            families: Vec::new(),
            series: Vec::new(),
            samples,
            series_samples,
            selected_series_keys,
            families_next_cursor: None,
            series_next_cursor: None,
            selected_family: 0,
            selected_series: 0,
        });
    }
    let series_response = client
        .series(&LocalMetricSeriesRequest {
            source: query.source,
            scope: scope.clone(),
            range,
            family: family_name.clone(),
            cursor: query.series_cursor.clone(),
            limit: None,
        })
        .await?;
    if query.load == MetricsLoad::AppendSeries {
        return Ok(MetricsRefreshPayload {
            capability,
            source: query.source,
            mode: query.mode,
            load: query.load,
            time_range: range,
            dashboard: None,
            families: Vec::new(),
            series: series_response.values,
            samples: Vec::new(),
            series_samples: Vec::new(),
            selected_series_keys: Vec::new(),
            families_next_cursor: None,
            series_next_cursor: series_response.next_cursor,
            selected_family: 0,
            selected_series: 0,
        });
    }
    let series_next_cursor = series_response.next_cursor;
    let series = series_response.values;
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
    let mut selected_series_keys = query
        .selected_series_keys
        .into_iter()
        .filter(|selected| series.iter().any(|series| series.series_key == *selected))
        .take(MAX_EXPLORE_SERIES)
        .collect::<Vec<_>>();
    if selected_series_keys.is_empty()
        && let Some(series) = series.get(selected_series)
    {
        selected_series_keys.push(series.series_key.clone());
    }
    let series_samples = load_series_samples(
        &client,
        query.source,
        &scope,
        range,
        &family_name,
        &selected_series_keys,
    )
    .await?;
    let samples = series_samples
        .iter()
        .find(|samples| {
            series
                .get(selected_series)
                .is_some_and(|series| series.series_key == samples.series_key)
        })
        .or_else(|| series_samples.first())
        .map(|samples| samples.response.samples.clone())
        .unwrap_or_default();

    Ok(MetricsRefreshPayload {
        capability,
        source: query.source,
        mode: query.mode,
        load: query.load,
        time_range: range,
        dashboard: None,
        families,
        series,
        samples,
        series_samples,
        selected_series_keys,
        families_next_cursor,
        series_next_cursor,
        selected_family,
        selected_series,
    })
}

async fn load_series_samples(
    client: &LocalObservabilityClient,
    source: LocalMetricSource,
    scope: &LocalMetricScope,
    range: LocalMetricTimeRange,
    family: &str,
    series_keys: &[String],
) -> anyhow::Result<Vec<MetricsSeriesSamples>> {
    let mut result = Vec::with_capacity(series_keys.len());
    for series_key in series_keys {
        let response = client
            .samples(&LocalMetricSamplesRequest {
                source,
                scope: scope.clone(),
                range,
                family: family.to_string(),
                series_key: series_key.clone(),
                max_points: None,
            })
            .await?;
        result.push(MetricsSeriesSamples {
            series_key: series_key.clone(),
            response,
        });
    }
    Ok(result)
}

fn current_range(preset: MetricsRange) -> LocalMetricTimeRange {
    let to = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    LocalMetricTimeRange {
        from_unix_nanos: to.saturating_sub(preset.duration().as_nanos() as i64),
        to_unix_nanos: to,
    }
}

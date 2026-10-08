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

use anyhow::{Context, anyhow};
use golem_cli::model::local_observability::{
    LOCAL_OBSERVABILITY_CAPABILITY_PATH, LOCAL_OBSERVABILITY_FAMILIES_PATH,
    LOCAL_OBSERVABILITY_SAMPLES_PATH, LOCAL_OBSERVABILITY_SERIES_PATH, LOCAL_OTLP_METRICS_PATH,
    LocalMetricFamiliesRequest, LocalMetricFamiliesResponse, LocalMetricFamily, LocalMetricKind,
    LocalMetricSample, LocalMetricSamplesRequest, LocalMetricSamplesResponse, LocalMetricScope,
    LocalMetricSeries, LocalMetricSeriesRequest, LocalMetricSeriesResponse, LocalMetricSource,
    LocalMetricTemporality, LocalObservabilityCapability, LocalObservabilityStatus,
};
use poem::http::{HeaderValue, StatusCode, header};
use poem::{Body, Request, Response, Route};
use prometheus::Registry;
use prometheus::proto::MetricType;
use serde::Serialize;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

pub const OBSERVABILITY_DB_FILE_NAME: &str = "observability.db";
const SCHEMA_VERSION: i64 = 2;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const RETENTION_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const RETENTION_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_FAMILY_LIMIT: u16 = 100;
const MAX_FAMILY_LIMIT: u16 = 500;
const DEFAULT_SERIES_LIMIT: u16 = 200;
const MAX_SERIES_LIMIT: u16 = 500;
const DEFAULT_SAMPLE_LIMIT: u16 = 240;
const MAX_SAMPLE_LIMIT: u16 = 1000;
const EXPORTER_NAME: &str = "golem-otlp-exporter";
const EXPORTER_VERSION: &str = "1.6.0";

#[derive(Clone, Debug)]
enum RuntimeState {
    Ready,
    Disabled,
    Failed(String),
}

pub struct LocalObservability {
    db_path: PathBuf,
    pool: Option<SqlitePool>,
    state: RwLock<RuntimeState>,
    discarded_points: AtomicU64,
    last_service_sample: AtomicI64,
    last_agent_ingest: AtomicI64,
}

#[derive(Debug)]
struct StoredPoint {
    source: LocalMetricSource,
    family: String,
    description: String,
    unit: String,
    kind: LocalMetricKind,
    temporality: LocalMetricTemporality,
    category: String,
    timestamp_ns: i64,
    value: f64,
    statistic: String,
    environment_id: Option<String>,
    component_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    attributes: BTreeMap<String, String>,
    source_oplog_index: Option<u64>,
}

impl LocalObservability {
    pub async fn start(
        enabled: bool,
        data_dir: &Path,
        registry: Registry,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Arc<Self> {
        let db_path = data_dir.join(OBSERVABILITY_DB_FILE_NAME);
        if !enabled {
            return Arc::new(Self {
                db_path,
                pool: None,
                state: RwLock::new(RuntimeState::Disabled),
                discarded_points: AtomicU64::new(0),
                last_service_sample: AtomicI64::new(0),
                last_agent_ingest: AtomicI64::new(0),
            });
        }

        match open_store(&db_path).await {
            Ok(pool) => {
                let service = Arc::new(Self {
                    db_path,
                    pool: Some(pool),
                    state: RwLock::new(RuntimeState::Ready),
                    discarded_points: AtomicU64::new(0),
                    last_service_sample: AtomicI64::new(0),
                    last_agent_ingest: AtomicI64::new(0),
                });
                let sampler = service.clone();
                join_set.spawn(async move {
                    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        if let Err(error) = sampler.sample_services(&registry).await {
                            sampler
                                .fail(format!("service metric sampling failed: {error:#}"))
                                .await;
                        }
                    }
                    #[allow(unreachable_code)]
                    Ok(())
                });
                service
            }
            Err(error) => {
                warn!(path = %db_path.display(), error = %error, "Local observability is unavailable");
                Arc::new(Self {
                    db_path,
                    pool: None,
                    state: RwLock::new(RuntimeState::Failed(error.to_string())),
                    discarded_points: AtomicU64::new(0),
                    last_service_sample: AtomicI64::new(0),
                    last_agent_ingest: AtomicI64::new(0),
                })
            }
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub async fn capability(&self) -> LocalObservabilityCapability {
        let (status, message) = match &*self.state.read().await {
            RuntimeState::Ready => (LocalObservabilityStatus::Ready, None),
            RuntimeState::Disabled => (
                LocalObservabilityStatus::Disabled,
                Some("Local metrics collection is disabled".to_string()),
            ),
            RuntimeState::Failed(message) => {
                (LocalObservabilityStatus::Failed, Some(message.clone()))
            }
        };
        LocalObservabilityCapability {
            status,
            message,
            exporter_name: EXPORTER_NAME.to_string(),
            exporter_version: EXPORTER_VERSION.to_string(),
            sample_interval_seconds: SAMPLE_INTERVAL.as_secs(),
            retention_days: RETENTION_AGE.as_secs() / (24 * 60 * 60),
            retention_bytes: RETENTION_BYTES,
            discarded_points: self.discarded_points.load(Ordering::Relaxed),
            last_service_sample_unix_nanos: non_zero(
                self.last_service_sample.load(Ordering::Relaxed),
            ),
            last_agent_ingest_unix_nanos: non_zero(self.last_agent_ingest.load(Ordering::Relaxed)),
        }
    }

    async fn fail(&self, message: String) {
        warn!(error = %message, "Local observability entered failed state");
        *self.state.write().await = RuntimeState::Failed(message);
    }

    async fn ready_pool(&self) -> anyhow::Result<&SqlitePool> {
        match &*self.state.read().await {
            RuntimeState::Ready => self
                .pool
                .as_ref()
                .ok_or_else(|| anyhow!("local observability store is unavailable")),
            RuntimeState::Disabled => Err(anyhow!("local observability is disabled")),
            RuntimeState::Failed(message) => Err(anyhow!(message.clone())),
        }
    }

    pub async fn ingest_otlp_json(
        &self,
        body: &[u8],
        environment_id: Option<&str>,
    ) -> OtlpIngestResult {
        let batch = match parse_otlp_json(body, environment_id) {
            Ok(batch) => batch,
            Err(error) => {
                return OtlpIngestResult::rejected(0, error.to_string());
            }
        };
        if batch.rejected > 0 {
            self.discarded_points
                .fetch_add(batch.rejected, Ordering::Relaxed);
        }
        if batch.points.is_empty() {
            return if batch.rejected == 0 {
                OtlpIngestResult::accepted()
            } else {
                OtlpIngestResult::rejected(
                    batch.rejected,
                    "points require valid Golem component and Agent identity and a supported numeric sum or gauge"
                        .to_string(),
                )
            };
        }
        let point_count = batch.points.len() as u64;
        let pool = match self.ready_pool().await {
            Ok(pool) => pool,
            Err(error) => {
                self.discarded_points
                    .fetch_add(point_count, Ordering::Relaxed);
                return OtlpIngestResult::rejected(point_count, error.to_string());
            }
        };
        if let Err(error) = insert_points(pool, &batch.points).await {
            self.discarded_points
                .fetch_add(point_count, Ordering::Relaxed);
            self.fail(format!("agent metric persistence failed: {error:#}"))
                .await;
            return OtlpIngestResult::rejected(point_count, error.to_string());
        }
        self.last_agent_ingest.store(now_ns(), Ordering::Relaxed);
        if batch.rejected == 0 {
            OtlpIngestResult::accepted()
        } else {
            OtlpIngestResult::rejected(
                batch.rejected,
                "some points lacked valid Golem identity or used an unsupported metric type"
                    .to_string(),
            )
        }
    }

    async fn sample_services(&self, registry: &Registry) -> anyhow::Result<()> {
        let pool = self.ready_pool().await?;
        let timestamp_ns = now_ns();
        let mut points = Vec::new();
        for family in registry.gather() {
            let kind = match family.type_() {
                MetricType::COUNTER => LocalMetricKind::Counter,
                MetricType::GAUGE => LocalMetricKind::Gauge,
                MetricType::HISTOGRAM => LocalMetricKind::Histogram,
                _ => continue,
            };
            for metric in family.get_metric() {
                let attributes = metric
                    .get_label()
                    .iter()
                    .map(|label| (label.name().to_string(), label.value().to_string()))
                    .collect::<BTreeMap<_, _>>();
                match kind {
                    LocalMetricKind::Counter => points.push(service_point(
                        family.name(),
                        family.help(),
                        kind,
                        timestamp_ns,
                        metric.get_counter().value(),
                        "value",
                        attributes,
                    )),
                    LocalMetricKind::Gauge => points.push(service_point(
                        family.name(),
                        family.help(),
                        kind,
                        timestamp_ns,
                        metric.get_gauge().value(),
                        "value",
                        attributes,
                    )),
                    LocalMetricKind::Histogram => {
                        let histogram = metric.get_histogram();
                        points.push(service_point(
                            family.name(),
                            family.help(),
                            kind,
                            timestamp_ns,
                            histogram.get_sample_count() as f64,
                            "count",
                            attributes.clone(),
                        ));
                        points.push(service_point(
                            family.name(),
                            family.help(),
                            kind,
                            timestamp_ns,
                            histogram.get_sample_sum(),
                            "sum",
                            attributes.clone(),
                        ));
                        for bucket in histogram.get_bucket() {
                            let mut bucket_attributes = attributes.clone();
                            bucket_attributes
                                .insert("le".to_string(), bucket.upper_bound().to_string());
                            points.push(service_point(
                                family.name(),
                                family.help(),
                                kind,
                                timestamp_ns,
                                bucket.cumulative_count() as f64,
                                "bucket",
                                bucket_attributes,
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        let point_count = points.len();
        points.retain(|point| point.value.is_finite());
        self.discarded_points.fetch_add(
            point_count.saturating_sub(points.len()) as u64,
            Ordering::Relaxed,
        );
        insert_points(pool, &points).await?;
        prune(pool).await?;
        self.last_service_sample
            .store(timestamp_ns, Ordering::Relaxed);
        Ok(())
    }

    pub async fn families(
        &self,
        request: &LocalMetricFamiliesRequest,
    ) -> anyhow::Result<LocalMetricFamiliesResponse> {
        let pool = self.ready_pool().await?;
        validate_range(request.range.from_unix_nanos, request.range.to_unix_nanos)?;
        let limit = request
            .limit
            .unwrap_or(DEFAULT_FAMILY_LIMIT)
            .clamp(1, MAX_FAMILY_LIMIT) as usize;
        let cursor = decode_cursor(&request.cursor)?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH scoped AS (SELECT id, family, description, unit, kind, temporality, category, timestamp_ns, value, statistic, series_key FROM metric_points WHERE 1 = 1",
        );
        push_metric_filters(
            &mut query,
            request.source,
            &request.scope,
            request.range.from_unix_nanos,
            request.range.to_unix_nanos,
            None,
            None,
        );
        if let Some(name_filter) = &request.name_filter {
            query
                .push(" AND INSTR(LOWER(family), LOWER(")
                .push_bind(name_filter.clone())
                .push(")) > 0");
        }
        query
            .push(
                "), ranked AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY family ORDER BY CASE WHEN statistic = 'value' THEN 0 ELSE 1 END, timestamp_ns DESC, id DESC) AS family_rank FROM scoped), aggregated AS (SELECT family, MAX(description) AS description, MAX(unit) AS unit, MAX(kind) AS kind, MAX(temporality) AS temporality, MAX(category) AS category, COUNT(DISTINCT series_key) AS series_count, MAX(CASE WHEN statistic = 'value' THEN timestamp_ns END) AS latest_unix_nanos FROM scoped GROUP BY family) SELECT aggregated.family, aggregated.description, aggregated.unit, aggregated.kind, aggregated.temporality, aggregated.category, aggregated.series_count, ranked.value AS latest_value, aggregated.latest_unix_nanos FROM aggregated LEFT JOIN ranked ON ranked.family = aggregated.family AND ranked.family_rank = 1 AND ranked.statistic = 'value' ORDER BY CASE WHEN aggregated.category IN ('Invocations', 'Execution') THEN 0 WHEN aggregated.category = 'Reliability' THEN 1 WHEN aggregated.category IN ('Runtime', 'Capacity') THEN 2 WHEN aggregated.category IN ('State Changes', 'Scheduling/Durability') THEN 3 WHEN aggregated.category = 'Compilation/Storage' THEN 4 ELSE 5 END, aggregated.family LIMIT ",
            )
            .push_bind((limit + 1) as i64)
            .push(" OFFSET ")
            .push_bind(cursor as i64);
        let mut values = query
            .build()
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(LocalMetricFamily {
                    name: row.try_get("family")?,
                    description: row.try_get("description")?,
                    unit: row.try_get("unit")?,
                    kind: parse_kind(row.try_get("kind")?),
                    temporality: parse_temporality(row.try_get("temporality")?),
                    category: row.try_get("category")?,
                    series_count: row.try_get::<i64, _>("series_count")?.max(0) as u64,
                    latest_value: row.try_get("latest_value")?,
                    latest_unix_nanos: row.try_get("latest_unix_nanos")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let next_cursor = (values.len() > limit).then(|| (cursor + limit).to_string());
        values.truncate(limit);
        Ok(LocalMetricFamiliesResponse {
            values,
            next_cursor,
        })
    }

    pub async fn series(
        &self,
        request: &LocalMetricSeriesRequest,
    ) -> anyhow::Result<LocalMetricSeriesResponse> {
        let pool = self.ready_pool().await?;
        validate_range(request.range.from_unix_nanos, request.range.to_unix_nanos)?;
        let limit = request
            .limit
            .unwrap_or(DEFAULT_SERIES_LIMIT)
            .clamp(1, MAX_SERIES_LIMIT) as usize;
        let cursor = decode_cursor(&request.cursor)?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH ranked AS (SELECT id, series_key, statistic, temporality, attributes_json, environment_id, component_id, agent_id, agent_type, value, timestamp_ns, ROW_NUMBER() OVER (PARTITION BY series_key ORDER BY timestamp_ns DESC, id DESC) AS series_rank FROM metric_points WHERE 1 = 1",
        );
        push_metric_filters(
            &mut query,
            request.source,
            &request.scope,
            request.range.from_unix_nanos,
            request.range.to_unix_nanos,
            Some(&request.family),
            None,
        );
        query
            .push(") SELECT series_key, statistic, temporality, attributes_json, environment_id, component_id, agent_id, agent_type, value, timestamp_ns FROM ranked WHERE series_rank = 1 ORDER BY series_key LIMIT ")
            .push_bind((limit + 1) as i64)
            .push(" OFFSET ")
            .push_bind(cursor as i64);
        let mut values = query
            .build()
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                let attributes_json: String = row.try_get("attributes_json")?;
                let environment_id: Option<String> = row.try_get("environment_id")?;
                let component_id: Option<String> = row.try_get("component_id")?;
                Ok(LocalMetricSeries {
                    series_key: row.try_get("series_key")?,
                    statistic: row.try_get("statistic")?,
                    temporality: parse_temporality(row.try_get("temporality")?),
                    attributes: serde_json::from_str(&attributes_json)?,
                    environment_id: environment_id.as_deref().and_then(parse_uuid),
                    component_id: component_id.as_deref().and_then(parse_uuid),
                    agent_id: row.try_get("agent_id")?,
                    agent_type: row.try_get("agent_type")?,
                    latest_value: row.try_get("value")?,
                    latest_unix_nanos: row.try_get("timestamp_ns")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let next_cursor = (values.len() > limit).then(|| (cursor + limit).to_string());
        values.truncate(limit);
        Ok(LocalMetricSeriesResponse {
            values,
            next_cursor,
        })
    }

    pub async fn samples(
        &self,
        request: &LocalMetricSamplesRequest,
    ) -> anyhow::Result<LocalMetricSamplesResponse> {
        let pool = self.ready_pool().await?;
        validate_range(request.range.from_unix_nanos, request.range.to_unix_nanos)?;
        let max_points = request
            .max_points
            .unwrap_or(DEFAULT_SAMPLE_LIMIT)
            .clamp(1, MAX_SAMPLE_LIMIT) as usize;
        let summary = sample_summary(
            pool,
            request.source,
            &request.scope,
            request.range.from_unix_nanos,
            request.range.to_unix_nanos,
            &request.family,
            &request.series_key,
        )
        .await?;
        let Some(profile) = sample_profile(
            pool,
            request.source,
            &request.scope,
            request.range.to_unix_nanos,
            &request.family,
            &request.series_key,
        )
        .await?
        else {
            return Ok(LocalMetricSamplesResponse {
                samples: Vec::new(),
                observed_from_unix_nanos: None,
                observed_to_unix_nanos: None,
                original_point_count: 0,
                approximate: false,
            });
        };
        let include_seed = profile.kind == LocalMetricKind::Gauge;
        let needs_seed = include_seed
            || (matches!(
                profile.kind,
                LocalMetricKind::Counter | LocalMetricKind::Histogram
            ) && profile.temporality == LocalMetricTemporality::Cumulative);
        let seed = if needs_seed {
            sample_seed(
                pool,
                request.source,
                &request.scope,
                request.range.from_unix_nanos,
                &request.family,
                &request.series_key,
            )
            .await?
        } else {
            None
        };
        let in_range_limit = max_points
            .saturating_sub(usize::from(include_seed && seed.is_some()))
            .max(1);
        let aggregate = summary.point_count as usize > in_range_limit;
        let mut samples = if aggregate {
            aggregate_samples(
                pool,
                request,
                profile,
                in_range_limit,
                seed.as_ref().map(|sample| sample.value),
            )
            .await?
        } else {
            raw_samples(pool, request, in_range_limit).await?
        };
        if include_seed && let Some(mut seed) = seed {
            seed.timestamp_unix_nanos = request.range.from_unix_nanos;
            samples.insert(0, seed);
            samples.truncate(max_points);
        }
        Ok(LocalMetricSamplesResponse {
            samples,
            observed_from_unix_nanos: summary.observed_from,
            observed_to_unix_nanos: summary.observed_to,
            original_point_count: summary.point_count,
            approximate: aggregate,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpIngestResult {
    partial_success: Option<OtlpPartialSuccess>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpPartialSuccess {
    rejected_data_points: String,
    error_message: String,
}

impl OtlpIngestResult {
    fn accepted() -> Self {
        Self {
            partial_success: None,
        }
    }

    fn rejected(count: u64, message: String) -> Self {
        Self {
            partial_success: Some(OtlpPartialSuccess {
                rejected_data_points: count.to_string(),
                error_message: message,
            }),
        }
    }
}

pub fn add_routes(
    app: Route,
    service: Arc<LocalObservability>,
    authorization_token: &'static str,
) -> Route {
    let capability = service.clone();
    let families = service.clone();
    let series = service.clone();
    let samples = service.clone();
    let ingest = service;
    app.at(
        LOCAL_OBSERVABILITY_CAPABILITY_PATH,
        poem::get(poem::endpoint::make(move |request: Request| {
            let service = capability.clone();
            async move {
                if !authorized(&request, authorization_token) {
                    return unauthorized();
                }
                json_response(StatusCode::OK, &service.capability().await)
            }
        })),
    )
    .at(
        LOCAL_OBSERVABILITY_FAMILIES_PATH,
        poem::post(poem::endpoint::make(move |request: Request| {
            let service = families.clone();
            async move {
                query_response::<LocalMetricFamiliesRequest, _, _>(
                    request,
                    authorization_token,
                    |request| async move { service.families(&request).await },
                )
                .await
            }
        })),
    )
    .at(
        LOCAL_OBSERVABILITY_SERIES_PATH,
        poem::post(poem::endpoint::make(move |request: Request| {
            let service = series.clone();
            async move {
                query_response::<LocalMetricSeriesRequest, _, _>(
                    request,
                    authorization_token,
                    |request| async move { service.series(&request).await },
                )
                .await
            }
        })),
    )
    .at(
        LOCAL_OBSERVABILITY_SAMPLES_PATH,
        poem::post(poem::endpoint::make(move |request: Request| {
            let service = samples.clone();
            async move {
                query_response::<LocalMetricSamplesRequest, _, _>(
                    request,
                    authorization_token,
                    |request| async move { service.samples(&request).await },
                )
                .await
            }
        })),
    )
    .at(
        LOCAL_OTLP_METRICS_PATH,
        poem::post(poem::endpoint::make(move |request: Request| {
            let service = ingest.clone();
            async move {
                if !authorized(&request, authorization_token) {
                    return unauthorized();
                }
                let environment_id = request
                    .headers()
                    .get("x-golem-environment-id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string);
                let content_type_ok = request
                    .content_type()
                    .is_some_and(|value| value.starts_with("application/json"));
                if !content_type_ok {
                    return json_error(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        "Only OTLP/HTTP JSON metrics are supported",
                    );
                }
                let body = match request.into_body().into_vec().await {
                    Ok(body) => body,
                    Err(error) => {
                        return json_error(StatusCode::BAD_REQUEST, error.to_string());
                    }
                };
                let result = service
                    .ingest_otlp_json(&body, environment_id.as_deref())
                    .await;
                json_response(StatusCode::OK, &result)
            }
        })),
    )
}

async fn query_response<T, R, U>(
    request: Request,
    authorization_token: &'static str,
    query: impl FnOnce(T) -> R,
) -> Response
where
    T: serde::de::DeserializeOwned,
    R: std::future::Future<Output = anyhow::Result<U>>,
    U: Serialize,
{
    if !authorized(&request, authorization_token) {
        return unauthorized();
    }
    let body = match request.into_body().into_vec().await {
        Ok(body) => body,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let request = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, error.to_string()),
    };
    match query(request).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => json_error(StatusCode::SERVICE_UNAVAILABLE, error.to_string()),
    }
}

fn authorized(request: &Request, token: &str) -> bool {
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {token}"))
}

fn unauthorized() -> Response {
    json_error(StatusCode::UNAUTHORIZED, "Unauthorized")
}

fn json_error(status: StatusCode, message: impl ToString) -> Response {
    json_response(status, &serde_json::json!({ "error": message.to_string() }))
}

fn json_response(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => Response::builder()
            .status(status)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(Body::from_vec(body)),
        Err(error) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(error.to_string()),
    }
}

async fn open_store(path: &Path) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&pool)
        .await?;
    if version != 0 && version != SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported observability database schema {version}; run `golem server clean --observability-only`"
        ));
    }
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS metric_points (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            family TEXT NOT NULL,
            description TEXT NOT NULL,
            unit TEXT NOT NULL,
            kind TEXT NOT NULL,
            temporality TEXT NOT NULL,
            category TEXT NOT NULL,
            timestamp_ns INTEGER NOT NULL,
            value REAL NOT NULL,
            statistic TEXT NOT NULL,
            environment_id TEXT,
            component_id TEXT,
            agent_id TEXT,
            agent_type TEXT,
            attributes_json TEXT NOT NULL,
            series_key TEXT NOT NULL,
            source_oplog_index TEXT
        )
        "#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS metric_points_lookup ON metric_points(source, family, timestamp_ns, series_key)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS metric_points_scope ON metric_points(environment_id, component_id, agent_type, agent_id)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .execute(&pool)
        .await?;
    info!(path = %path.display(), "Local observability store ready");
    Ok(pool)
}

pub async fn clear_store(path: &Path) -> anyhow::Result<()> {
    for candidate in [
        path.to_path_buf(),
        path_with_suffix(path, "-wal"),
        path_with_suffix(path, "-shm"),
    ] {
        match tokio::fs::remove_file(&candidate).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to delete {}", candidate.display()));
            }
        }
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

async fn insert_points(pool: &SqlitePool, points: &[StoredPoint]) -> anyhow::Result<()> {
    if points.is_empty() {
        return Ok(());
    }
    let mut transaction = pool.begin().await?;
    for point in points {
        let attributes_json = serde_json::to_string(&point.attributes)?;
        let series_key = series_key(point, &attributes_json);
        sqlx::query(
            r#"
            INSERT INTO metric_points (
                source, family, description, unit, kind, temporality, category, timestamp_ns, value,
                statistic, environment_id, component_id, agent_id, agent_type,
                attributes_json, series_key, source_oplog_index
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(source_name(point.source))
        .bind(&point.family)
        .bind(&point.description)
        .bind(&point.unit)
        .bind(kind_name(point.kind))
        .bind(temporality_name(point.temporality))
        .bind(&point.category)
        .bind(point.timestamp_ns)
        .bind(point.value)
        .bind(&point.statistic)
        .bind(&point.environment_id)
        .bind(&point.component_id)
        .bind(&point.agent_id)
        .bind(&point.agent_type)
        .bind(attributes_json)
        .bind(series_key)
        .bind(point.source_oplog_index.map(|index| index.to_string()))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn prune(pool: &SqlitePool) -> anyhow::Result<()> {
    let cutoff = now_ns().saturating_sub(RETENTION_AGE.as_nanos() as i64);
    sqlx::query("DELETE FROM metric_points WHERE timestamp_ns < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;

    while store_allocated_bytes(pool).await? > RETENTION_BYTES {
        let deleted = sqlx::query(
            "DELETE FROM metric_points WHERE id IN (SELECT id FROM metric_points ORDER BY timestamp_ns, id LIMIT 10000)",
        )
        .execute(pool)
        .await?
        .rows_affected();
        if deleted == 0 {
            break;
        }
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(pool)
            .await?;
        sqlx::query("VACUUM").execute(pool).await?;
    }
    Ok(())
}

async fn store_allocated_bytes(pool: &SqlitePool) -> anyhow::Result<u64> {
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(pool)
        .await?;
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(pool)
        .await?;
    Ok((page_count as u64).saturating_mul(page_size as u64))
}

#[derive(Clone, Debug)]
struct SampleProfile {
    kind: LocalMetricKind,
    temporality: LocalMetricTemporality,
    statistic: String,
}

#[derive(Clone, Copy, Debug)]
struct SampleSummary {
    point_count: u64,
    observed_from: Option<i64>,
    observed_to: Option<i64>,
}

async fn sample_profile(
    pool: &SqlitePool,
    source: LocalMetricSource,
    scope: &LocalMetricScope,
    to: i64,
    family: &str,
    series_key: &str,
) -> anyhow::Result<Option<SampleProfile>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT kind, temporality, statistic FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        source,
        scope,
        i64::MIN,
        to,
        Some(family),
        Some(series_key),
    );
    query.push(" ORDER BY timestamp_ns DESC, id DESC LIMIT 1");
    let Some(row) = query.build().fetch_optional(pool).await? else {
        return Ok(None);
    };
    Ok(Some(SampleProfile {
        kind: parse_kind(row.try_get("kind")?),
        temporality: parse_temporality(row.try_get("temporality")?),
        statistic: row.try_get("statistic")?,
    }))
}

async fn sample_summary(
    pool: &SqlitePool,
    source: LocalMetricSource,
    scope: &LocalMetricScope,
    from: i64,
    to: i64,
    family: &str,
    series_key: &str,
) -> anyhow::Result<SampleSummary> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) AS point_count, MIN(timestamp_ns) AS observed_from, MAX(timestamp_ns) AS observed_to FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        source,
        scope,
        from,
        to,
        Some(family),
        Some(series_key),
    );
    let row = query.build().fetch_one(pool).await?;
    let point_count = row.try_get::<i64, _>("point_count")?.max(0) as u64;
    Ok(SampleSummary {
        point_count,
        observed_from: row.try_get("observed_from")?,
        observed_to: row.try_get("observed_to")?,
    })
}

async fn raw_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    limit: usize,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT timestamp_ns, value, statistic, source_oplog_index FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        request.source,
        &request.scope,
        request.range.from_unix_nanos,
        request.range.to_unix_nanos,
        Some(&request.family),
        Some(&request.series_key),
    );
    query
        .push(" ORDER BY timestamp_ns, id LIMIT ")
        .push_bind(limit as i64);
    query
        .build()
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(read_sample)
        .collect()
}

async fn sample_seed(
    pool: &SqlitePool,
    source: LocalMetricSource,
    scope: &LocalMetricScope,
    from: i64,
    family: &str,
    series_key: &str,
) -> anyhow::Result<Option<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT timestamp_ns, value, statistic, source_oplog_index FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        source,
        scope,
        i64::MIN,
        from,
        Some(family),
        Some(series_key),
    );
    query.push(" ORDER BY timestamp_ns DESC, id DESC LIMIT 1");
    query
        .build()
        .fetch_optional(pool)
        .await?
        .map(read_sample)
        .transpose()
}

async fn aggregate_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    profile: SampleProfile,
    max_points: usize,
    seed: Option<f64>,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let span = request
        .range
        .to_unix_nanos
        .saturating_sub(request.range.from_unix_nanos)
        .max(1);
    let divisor = max_points.max(1) as i64;
    let bucket_width = span.saturating_add(divisor - 1) / divisor;
    match (profile.kind, profile.temporality) {
        (LocalMetricKind::Gauge, _) => {
            aggregate_last_samples(pool, request, max_points, bucket_width).await
        }
        (_, LocalMetricTemporality::Delta) => {
            aggregate_sum_samples(pool, request, &profile.statistic, max_points, bucket_width).await
        }
        (
            LocalMetricKind::Counter | LocalMetricKind::Histogram,
            LocalMetricTemporality::Cumulative,
        ) => aggregate_cumulative_samples(pool, request, max_points, bucket_width, seed).await,
        (LocalMetricKind::Event, _) => {
            aggregate_mean_samples(pool, request, max_points, bucket_width).await
        }
        _ => aggregate_last_samples(pool, request, max_points, bucket_width).await,
    }
}

async fn aggregate_last_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    max_points: usize,
    bucket_width: i64,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "WITH ranked AS (SELECT timestamp_ns, value, statistic, source_oplog_index, ROW_NUMBER() OVER (PARTITION BY ((timestamp_ns - ",
    );
    query.push_bind(request.range.from_unix_nanos);
    query.push(") / ").push_bind(bucket_width);
    query.push(
        ") ORDER BY timestamp_ns DESC, id DESC) AS bucket_rank FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        request.source,
        &request.scope,
        request.range.from_unix_nanos,
        request.range.to_unix_nanos,
        Some(&request.family),
        Some(&request.series_key),
    );
    query
        .push(") SELECT timestamp_ns, value, statistic, source_oplog_index FROM ranked WHERE bucket_rank = 1 ORDER BY timestamp_ns LIMIT ")
        .push_bind(max_points as i64);
    query
        .build()
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(read_sample)
        .collect()
}

async fn aggregate_sum_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    statistic: &str,
    max_points: usize,
    bucket_width: i64,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new("WITH bucketed AS (SELECT ");
    push_bucket_timestamp(&mut query, request.range.from_unix_nanos, bucket_width);
    query.push(" AS bucket_timestamp, value FROM metric_points WHERE 1 = 1");
    push_metric_filters(
        &mut query,
        request.source,
        &request.scope,
        request.range.from_unix_nanos,
        request.range.to_unix_nanos,
        Some(&request.family),
        Some(&request.series_key),
    );
    query
        .push(") SELECT bucket_timestamp AS timestamp_ns, SUM(value) AS value FROM bucketed GROUP BY bucket_timestamp ORDER BY bucket_timestamp LIMIT ")
        .push_bind(max_points as i64);
    let rows = query.build().fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(LocalMetricSample {
                timestamp_unix_nanos: row.try_get("timestamp_ns")?,
                value: row.try_get("value")?,
                statistic: statistic.to_string(),
                source_oplog_index: None,
            })
        })
        .collect()
}

async fn aggregate_cumulative_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    max_points: usize,
    bucket_width: i64,
    seed: Option<f64>,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "WITH deltas AS (SELECT timestamp_ns, CASE WHEN previous_value IS NULL THEN NULL WHEN value >= previous_value THEN value - previous_value ELSE value END AS value FROM (SELECT timestamp_ns, value, LAG(value, 1, ",
    );
    query.push_bind(seed);
    query.push(
        ") OVER (ORDER BY timestamp_ns, id) AS previous_value FROM metric_points WHERE 1 = 1",
    );
    push_metric_filters(
        &mut query,
        request.source,
        &request.scope,
        request.range.from_unix_nanos,
        request.range.to_unix_nanos,
        Some(&request.family),
        Some(&request.series_key),
    );
    query.push(")), bucketed AS (SELECT ");
    push_bucket_timestamp(&mut query, request.range.from_unix_nanos, bucket_width);
    query.push(" AS bucket_timestamp, value FROM deltas WHERE value IS NOT NULL)");
    query
        .push(" SELECT bucket_timestamp AS timestamp_ns, SUM(value) AS value FROM bucketed GROUP BY bucket_timestamp ORDER BY bucket_timestamp LIMIT ")
        .push_bind(max_points as i64);
    let rows = query.build().fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(LocalMetricSample {
                timestamp_unix_nanos: row.try_get("timestamp_ns")?,
                value: row.try_get("value")?,
                statistic: "delta".to_string(),
                source_oplog_index: None,
            })
        })
        .collect()
}

async fn aggregate_mean_samples(
    pool: &SqlitePool,
    request: &LocalMetricSamplesRequest,
    max_points: usize,
    bucket_width: i64,
) -> anyhow::Result<Vec<LocalMetricSample>> {
    let mut query = QueryBuilder::<Sqlite>::new("WITH bucketed AS (SELECT ");
    push_bucket_timestamp(&mut query, request.range.from_unix_nanos, bucket_width);
    query.push(" AS bucket_timestamp, value FROM metric_points WHERE 1 = 1");
    push_metric_filters(
        &mut query,
        request.source,
        &request.scope,
        request.range.from_unix_nanos,
        request.range.to_unix_nanos,
        Some(&request.family),
        Some(&request.series_key),
    );
    query
        .push(") SELECT bucket_timestamp AS timestamp_ns, AVG(value) AS value FROM bucketed GROUP BY bucket_timestamp ORDER BY bucket_timestamp LIMIT ")
        .push_bind(max_points as i64);
    let rows = query.build().fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(LocalMetricSample {
                timestamp_unix_nanos: row.try_get("timestamp_ns")?,
                value: row.try_get("value")?,
                statistic: "mean".to_string(),
                source_oplog_index: None,
            })
        })
        .collect()
}

fn push_bucket_timestamp(
    query: &mut QueryBuilder<'_, Sqlite>,
    range_start: i64,
    bucket_width: i64,
) {
    query.push_bind(range_start);
    query.push(" + ((timestamp_ns - ");
    query.push_bind(range_start);
    query.push(") / ").push_bind(bucket_width);
    query.push(") * ").push_bind(bucket_width);
}

fn push_metric_filters(
    query: &mut QueryBuilder<'_, Sqlite>,
    source: LocalMetricSource,
    scope: &LocalMetricScope,
    from: i64,
    to: i64,
    family: Option<&str>,
    series_key: Option<&str>,
) {
    query.push(" AND source = ").push_bind(source_name(source));
    query.push(" AND timestamp_ns >= ").push_bind(from);
    query.push(" AND timestamp_ns < ").push_bind(to);
    if let Some(family) = family {
        query.push(" AND family = ").push_bind(family.to_string());
    }
    if let Some(series_key) = series_key {
        query
            .push(" AND series_key = ")
            .push_bind(series_key.to_string());
    }
    if let Some(environment_id) = scope.environment_id {
        query
            .push(" AND (environment_id = ")
            .push_bind(environment_id.to_string())
            .push(" OR environment_id IS NULL)");
    }
    push_string_filter(
        query,
        "component_id",
        scope.component_ids.iter().map(Uuid::to_string),
    );
    push_string_filter(query, "agent_id", scope.agent_ids.iter().cloned());
    if let Some(agent_type) = &scope.agent_type {
        query
            .push(" AND agent_type = ")
            .push_bind(agent_type.clone());
    }
}

fn read_sample(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<LocalMetricSample> {
    Ok(LocalMetricSample {
        timestamp_unix_nanos: row.try_get("timestamp_ns")?,
        value: row.try_get("value")?,
        statistic: row.try_get("statistic")?,
        source_oplog_index: row
            .try_get::<Option<String>, _>("source_oplog_index")?
            .and_then(|index| index.parse().ok()),
    })
}

fn push_string_filter(
    query: &mut QueryBuilder<'_, Sqlite>,
    column: &str,
    values: impl IntoIterator<Item = String>,
) {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return;
    }
    query.push(" AND ").push(column).push(" IN (");
    let mut separated = query.separated(", ");
    for value in values {
        separated.push_bind(value);
    }
    separated.push_unseparated(")");
}

struct ParsedOtlpBatch {
    points: Vec<StoredPoint>,
    rejected: u64,
}

fn parse_otlp_json(body: &[u8], environment_id: Option<&str>) -> anyhow::Result<ParsedOtlpBatch> {
    let root: Value = serde_json::from_slice(body).context("invalid OTLP JSON")?;
    let resource_metrics = root
        .get("resourceMetrics")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("missing resourceMetrics"))?;
    let environment_id = environment_id
        .map(Uuid::parse_str)
        .transpose()
        .context("invalid x-golem-environment-id")?
        .map(|id| id.to_string());
    let mut points = Vec::new();
    let mut rejected = 0u64;
    for resource_metric in resource_metrics {
        let resource_attributes = parse_attributes(
            resource_metric
                .pointer("/resource/attributes")
                .and_then(Value::as_array),
        );
        let component_id = resource_attributes
            .get("golem.component.id")
            .and_then(|value| Uuid::parse_str(value).ok());
        let agent_id = resource_attributes.get("golem.agent.id").cloned();
        let identity = component_id
            .zip(agent_id)
            .and_then(|(component_id, agent_id)| {
                let prefix = format!("{component_id}/");
                let agent_type = agent_type_from_agent_name(agent_id.strip_prefix(&prefix)?);
                Some((component_id, agent_id, agent_type))
            });
        let Some((component_id, agent_id, agent_type)) = identity else {
            rejected += count_data_points(resource_metric);
            continue;
        };
        let Some(scopes) = resource_metric
            .get("scopeMetrics")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for scope in scopes {
            let Some(metrics) = scope.get("metrics").and_then(Value::as_array) else {
                continue;
            };
            for metric in metrics {
                let family = metric
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if family.is_empty() {
                    rejected += count_metric_points(metric);
                    continue;
                }
                let description = metric
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let unit = metric
                    .get("unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let (kind, temporality, data_points) = if let Some(sum) = metric.get("sum") {
                    let monotonic = sum
                        .get("isMonotonic")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let kind = known_agent_kind(family, monotonic);
                    let temporality =
                        match sum.get("aggregationTemporality").and_then(parse_i64_value) {
                            Some(1) => LocalMetricTemporality::Delta,
                            Some(2) => LocalMetricTemporality::Cumulative,
                            _ => LocalMetricTemporality::Unspecified,
                        };
                    (
                        kind,
                        temporality,
                        sum.get("dataPoints").and_then(Value::as_array),
                    )
                } else if let Some(gauge) = metric.get("gauge") {
                    (
                        LocalMetricKind::Gauge,
                        LocalMetricTemporality::Unspecified,
                        gauge.get("dataPoints").and_then(Value::as_array),
                    )
                } else {
                    rejected += count_metric_points(metric);
                    continue;
                };
                let Some(data_points) = data_points else {
                    continue;
                };
                for data_point in data_points {
                    let Some(timestamp_ns) = parse_i64(data_point.get("timeUnixNano")) else {
                        rejected += 1;
                        continue;
                    };
                    let Some(value) = parse_number(data_point) else {
                        rejected += 1;
                        continue;
                    };
                    let mut attributes = resource_attributes.clone();
                    attributes.extend(parse_attributes(
                        data_point.get("attributes").and_then(Value::as_array),
                    ));
                    let source_oplog_index = attributes
                        .get("golem.oplog.index")
                        .and_then(|index| index.parse::<u64>().ok());
                    attributes.retain(|key, _| !reserved_correlation_attribute(key));
                    points.push(StoredPoint {
                        source: LocalMetricSource::Agents,
                        family: family.to_string(),
                        description: description.to_string(),
                        unit: unit.to_string(),
                        kind,
                        temporality,
                        category: agent_category(family).to_string(),
                        timestamp_ns,
                        value,
                        statistic: "value".to_string(),
                        environment_id: environment_id.clone(),
                        component_id: Some(component_id.to_string()),
                        agent_id: Some(agent_id.clone()),
                        agent_type: agent_type.clone(),
                        attributes,
                        source_oplog_index,
                    });
                }
            }
        }
    }
    Ok(ParsedOtlpBatch { points, rejected })
}

fn count_data_points(resource_metric: &Value) -> u64 {
    resource_metric
        .get("scopeMetrics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|scope| {
            scope
                .get("metrics")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .map(count_metric_points)
        .sum()
}

fn count_metric_points(metric: &Value) -> u64 {
    [
        "sum",
        "gauge",
        "histogram",
        "exponentialHistogram",
        "summary",
    ]
    .into_iter()
    .filter_map(|kind| metric.get(kind))
    .filter_map(|kind| kind.get("dataPoints"))
    .filter_map(Value::as_array)
    .map(|points| points.len() as u64)
    .sum()
}

fn parse_attributes(values: Option<&Vec<Value>>) -> BTreeMap<String, String> {
    values
        .into_iter()
        .flatten()
        .filter_map(|attribute| {
            let key = attribute.get("key")?.as_str()?.to_string();
            let value = attribute.get("value")?;
            let value = [
                "stringValue",
                "intValue",
                "doubleValue",
                "boolValue",
                "bytesValue",
            ]
            .into_iter()
            .find_map(|field| value.get(field))?;
            Some((
                key,
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string()),
            ))
        })
        .collect()
}

fn parse_number(data_point: &Value) -> Option<f64> {
    data_point
        .get("asDouble")
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .or_else(|| {
            data_point.get("asInt").and_then(|value| {
                value
                    .as_i64()
                    .map(|v| v as f64)
                    .or_else(|| value.as_str()?.parse().ok())
            })
        })
        .filter(|value| value.is_finite())
}

fn parse_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(parse_i64_value)
}

fn parse_i64_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.parse().ok())
}

fn reserved_correlation_attribute(key: &str) -> bool {
    matches!(
        key,
        "service.name"
            | "golem.agent.id"
            | "golem.component.id"
            | "golem.component.version"
            | "golem.oplog.index"
    )
}

fn service_point(
    family: &str,
    description: &str,
    kind: LocalMetricKind,
    timestamp_ns: i64,
    value: f64,
    statistic: &str,
    attributes: BTreeMap<String, String>,
) -> StoredPoint {
    StoredPoint {
        source: LocalMetricSource::Services,
        family: family.to_string(),
        description: description.to_string(),
        unit: service_unit(family).to_string(),
        kind,
        temporality: match kind {
            LocalMetricKind::Counter | LocalMetricKind::Histogram => {
                LocalMetricTemporality::Cumulative
            }
            LocalMetricKind::Gauge | LocalMetricKind::Event | LocalMetricKind::Unknown => {
                LocalMetricTemporality::Unspecified
            }
        },
        category: service_category(family).to_string(),
        timestamp_ns,
        value,
        statistic: statistic.to_string(),
        environment_id: None,
        component_id: None,
        agent_id: None,
        agent_type: None,
        attributes,
        source_oplog_index: None,
    }
}

fn series_key(point: &StoredPoint, attributes_json: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        point.environment_id.as_deref().unwrap_or_default(),
        point.component_id.as_deref().unwrap_or_default(),
        point.agent_id.as_deref().unwrap_or_default(),
        point.agent_type.as_deref().unwrap_or_default(),
        temporality_name(point.temporality),
        point.statistic,
        attributes_json
    )
}

fn known_agent_kind(family: &str, monotonic: bool) -> LocalMetricKind {
    if matches!(
        family,
        "golem.invocation.duration_ns"
            | "golem.invocation.fuel_consumed"
            | "golem.memory.growth_bytes"
            | "golem.snapshot.size_bytes"
    ) {
        LocalMetricKind::Event
    } else if family == "golem.resources.active" || !monotonic {
        LocalMetricKind::Gauge
    } else {
        LocalMetricKind::Counter
    }
}

fn agent_category(family: &str) -> &'static str {
    if family.starts_with("golem.invocation") {
        "Invocations"
    } else if matches!(
        family,
        "golem.error.count"
            | "golem.interruption.count"
            | "golem.exit.count"
            | "golem.restart.count"
    ) {
        "Reliability"
    } else if family.starts_with("golem.memory")
        || family.starts_with("golem.resources")
        || family.starts_with("golem.host_call")
        || family.starts_with("golem.component")
    {
        "Runtime"
    } else if family.starts_with("golem.transaction")
        || family.starts_with("golem.update")
        || family.starts_with("golem.snapshot")
        || family.starts_with("golem.log")
        || family.starts_with("golem.oplog_processor")
    {
        "State Changes"
    } else {
        "Other"
    }
}

fn service_category(family: &str) -> &'static str {
    if family.contains("memory") || family.contains("worker_count") || family.contains("resident") {
        "Capacity"
    } else if family.contains("scheduler")
        || family.contains("scheduled")
        || family.contains("oplog")
    {
        "Scheduling/Durability"
    } else if family.contains("compilation") || family.contains("storage") {
        "Compilation/Storage"
    } else if family.contains("invocation")
        || family.contains("worker")
        || family.contains("entity")
    {
        "Execution"
    } else {
        "Other"
    }
}

fn service_unit(family: &str) -> &'static str {
    if family.ends_with("_seconds") || family.contains("_seconds_") {
        "s"
    } else if family.ends_with("_bytes") || family.contains("_bytes_") {
        "By"
    } else {
        "1"
    }
}

fn source_name(source: LocalMetricSource) -> &'static str {
    match source {
        LocalMetricSource::Services => "services",
        LocalMetricSource::Agents => "agents",
    }
}

fn kind_name(kind: LocalMetricKind) -> &'static str {
    match kind {
        LocalMetricKind::Counter => "counter",
        LocalMetricKind::Gauge => "gauge",
        LocalMetricKind::Histogram => "histogram",
        LocalMetricKind::Event => "event",
        LocalMetricKind::Unknown => "unknown",
    }
}

fn parse_kind(kind: String) -> LocalMetricKind {
    match kind.as_str() {
        "counter" => LocalMetricKind::Counter,
        "gauge" => LocalMetricKind::Gauge,
        "histogram" => LocalMetricKind::Histogram,
        "event" => LocalMetricKind::Event,
        _ => LocalMetricKind::Unknown,
    }
}

fn temporality_name(temporality: LocalMetricTemporality) -> &'static str {
    match temporality {
        LocalMetricTemporality::Unspecified => "unspecified",
        LocalMetricTemporality::Delta => "delta",
        LocalMetricTemporality::Cumulative => "cumulative",
    }
}

fn parse_temporality(temporality: String) -> LocalMetricTemporality {
    match temporality.as_str() {
        "delta" => LocalMetricTemporality::Delta,
        "cumulative" => LocalMetricTemporality::Cumulative,
        _ => LocalMetricTemporality::Unspecified,
    }
}

fn agent_type_from_agent_name(name: &str) -> Option<String> {
    name.split_once('(')
        .map(|(agent_type, _)| agent_type.trim())
        .filter(|agent_type| !agent_type.is_empty())
        .map(ToString::to_string)
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

fn non_zero(value: i64) -> Option<i64> {
    (value != 0).then_some(value)
}

fn parse_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value).ok()
}

fn decode_cursor(cursor: &Option<String>) -> anyhow::Result<usize> {
    cursor
        .as_deref()
        .unwrap_or("0")
        .parse()
        .context("invalid pagination cursor")
}

fn validate_range(from: i64, to: i64) -> anyhow::Result<()> {
    if from > to {
        Err(anyhow!("metric range start must not be after its end"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::test::TestClient;
    use test_r::test;

    #[test]
    fn parses_current_exporter_numeric_points_and_golem_identity() {
        let body = serde_json::json!({
            "resourceMetrics": [{
                "resource": {"attributes": [
                    {"key": "golem.component.id", "value": {"stringValue": "7f33028a-b204-4f23-92e5-12044622e86e"}},
                    {"key": "golem.agent.id", "value": {"stringValue": "7f33028a-b204-4f23-92e5-12044622e86e/Counter(\"a\")"}}
                ]},
                "scopeMetrics": [{"metrics": [{
                    "name": "golem.invocation.count",
                    "unit": "1",
                    "sum": {"aggregationTemporality": 1, "isMonotonic": true, "dataPoints": [{
                        "timeUnixNano": "42", "asInt": "1", "attributes": [
                            {"key": "golem.oplog.index", "value": {"stringValue": "9"}}
                        ]
                    }]}
                }]}]
            }]
        });
        let batch = parse_otlp_json(&serde_json::to_vec(&body).unwrap(), None).unwrap();
        assert_eq!(batch.points.len(), 1);
        assert_eq!(batch.rejected, 0);
        assert_eq!(batch.points[0].agent_type.as_deref(), Some("Counter"));
        assert_eq!(batch.points[0].kind, LocalMetricKind::Counter);
        assert_eq!(batch.points[0].temporality, LocalMetricTemporality::Delta);
        assert_eq!(batch.points[0].source_oplog_index, Some(9));
        assert!(!batch.points[0].attributes.contains_key("golem.oplog.index"));
    }

    #[test]
    fn rejects_points_without_golem_identity() {
        let body = serde_json::json!({
            "resourceMetrics": [{
                "resource": {"attributes": []},
                "scopeMetrics": [{"metrics": [{
                    "name": "other",
                    "gauge": {"dataPoints": [{"timeUnixNano": "42", "asInt": "1"}]}
                }]}]
            }]
        });
        let batch = parse_otlp_json(&serde_json::to_vec(&body).unwrap(), None).unwrap();
        assert!(batch.points.is_empty());
        assert_eq!(batch.rejected, 1);
    }

    #[test]
    fn histogram_statistics_use_distinct_series_keys() {
        let count = service_point(
            "request_duration",
            "Request duration",
            LocalMetricKind::Histogram,
            1,
            2.0,
            "count",
            BTreeMap::new(),
        );
        let sum = service_point(
            "request_duration",
            "Request duration",
            LocalMetricKind::Histogram,
            1,
            3.0,
            "sum",
            BTreeMap::new(),
        );
        let attributes = serde_json::to_string(&BTreeMap::<String, String>::new()).unwrap();

        assert_ne!(
            series_key(&count, &attributes),
            series_key(&sum, &attributes)
        );
    }

    #[test]
    async fn stores_queries_and_cleans_agent_metrics() {
        let test_dir =
            std::env::temp_dir().join(format!("golem-local-observability-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&test_dir).await.unwrap();
        let db_path = test_dir.join(OBSERVABILITY_DB_FILE_NAME);
        let pool = open_store(&db_path).await.unwrap();
        let environment_id = Uuid::new_v4();
        let component_id = Uuid::new_v4();
        let agent_id = format!("{component_id}/Counter(\"stored\")");
        let timestamp = now_ns();
        let point = StoredPoint {
            source: LocalMetricSource::Agents,
            family: "golem.invocation.count".to_string(),
            description: "Invocations".to_string(),
            unit: "1".to_string(),
            kind: LocalMetricKind::Counter,
            temporality: LocalMetricTemporality::Delta,
            category: "Invocations".to_string(),
            timestamp_ns: timestamp,
            value: 3.0,
            statistic: "value".to_string(),
            environment_id: Some(environment_id.to_string()),
            component_id: Some(component_id.to_string()),
            agent_id: Some(agent_id.clone()),
            agent_type: Some("Counter".to_string()),
            attributes: BTreeMap::new(),
            source_oplog_index: Some(17),
        };
        let attributes_json = serde_json::to_string(&point.attributes).unwrap();
        let stored_series_key = series_key(&point, &attributes_json);
        insert_points(&pool, &[point]).await.unwrap();

        let samples = raw_samples(
            &pool,
            &LocalMetricSamplesRequest {
                source: LocalMetricSource::Agents,
                scope: LocalMetricScope {
                    environment_id: Some(environment_id),
                    component_ids: Vec::new(),
                    agent_ids: Vec::new(),
                    agent_type: None,
                },
                range: golem_cli::model::local_observability::LocalMetricTimeRange {
                    from_unix_nanos: timestamp - 1,
                    to_unix_nanos: timestamp + 1,
                },
                family: "golem.invocation.count".to_string(),
                series_key: stored_series_key,
                max_points: Some(10),
            },
            10,
        )
        .await
        .unwrap();
        assert_eq!(samples[0].source_oplog_index, Some(17));

        pool.close().await;
        clear_store(&db_path).await.unwrap();
        assert!(!db_path.exists());
        tokio::fs::remove_dir(&test_dir).await.unwrap();
    }

    #[test]
    async fn sample_queries_use_truthful_bounded_transforms() {
        let test_dir = std::env::temp_dir().join(format!(
            "golem-local-observability-samples-test-{}",
            Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&test_dir).await.unwrap();
        let db_path = test_dir.join(OBSERVABILITY_DB_FILE_NAME);
        let pool = open_store(&db_path).await.unwrap();
        let environment_id = Uuid::new_v4();
        let component_id = Uuid::new_v4();
        let agent_id = format!("{component_id}/Counter(\"samples\")");
        let point = |family: &str,
                     kind: LocalMetricKind,
                     temporality: LocalMetricTemporality,
                     timestamp_ns: i64,
                     value: f64| StoredPoint {
            source: LocalMetricSource::Agents,
            family: family.to_string(),
            description: family.to_string(),
            unit: "1".to_string(),
            kind,
            temporality,
            category: "test".to_string(),
            timestamp_ns,
            value,
            statistic: "value".to_string(),
            environment_id: Some(environment_id.to_string()),
            component_id: Some(component_id.to_string()),
            agent_id: Some(agent_id.clone()),
            agent_type: Some("Counter".to_string()),
            attributes: BTreeMap::new(),
            source_oplog_index: Some(timestamp_ns as u64),
        };
        let mut points = Vec::new();
        for (timestamp, value) in [(10, 1.0), (20, 2.0), (30, 3.0), (40, 4.0)] {
            points.push(point(
                "delta",
                LocalMetricKind::Counter,
                LocalMetricTemporality::Delta,
                timestamp,
                value,
            ));
        }
        for (timestamp, value) in [(5, 2.0), (10, 5.0), (20, 8.0), (30, 2.0), (40, 7.0)] {
            points.push(point(
                "cumulative",
                LocalMetricKind::Counter,
                LocalMetricTemporality::Cumulative,
                timestamp,
                value,
            ));
        }
        for (timestamp, value) in [(5, 9.0), (20, 10.0), (40, 12.0)] {
            points.push(point(
                "gauge",
                LocalMetricKind::Gauge,
                LocalMetricTemporality::Unspecified,
                timestamp,
                value,
            ));
        }
        let attributes_json = serde_json::to_string(&points[0].attributes).unwrap();
        let delta_series_key = series_key(
            points.iter().find(|point| point.family == "delta").unwrap(),
            &attributes_json,
        );
        let cumulative_series_key = series_key(
            points
                .iter()
                .find(|point| point.family == "cumulative")
                .unwrap(),
            &attributes_json,
        );
        let gauge_series_key = series_key(
            points.iter().find(|point| point.family == "gauge").unwrap(),
            &attributes_json,
        );
        insert_points(&pool, &points).await.unwrap();
        let service = LocalObservability {
            db_path: db_path.clone(),
            pool: Some(pool.clone()),
            state: RwLock::new(RuntimeState::Ready),
            discarded_points: AtomicU64::new(0),
            last_service_sample: AtomicI64::new(0),
            last_agent_ingest: AtomicI64::new(0),
        };
        let request = |family: &str, series_key: &str, from: i64, to: i64, max_points: u16| {
            LocalMetricSamplesRequest {
                source: LocalMetricSource::Agents,
                scope: LocalMetricScope {
                    environment_id: Some(environment_id),
                    component_ids: Vec::new(),
                    agent_ids: Vec::new(),
                    agent_type: None,
                },
                range: golem_cli::model::local_observability::LocalMetricTimeRange {
                    from_unix_nanos: from,
                    to_unix_nanos: to,
                },
                family: family.to_string(),
                series_key: series_key.to_string(),
                max_points: Some(max_points),
            }
        };

        let families = service
            .families(&LocalMetricFamiliesRequest {
                source: LocalMetricSource::Agents,
                scope: LocalMetricScope {
                    environment_id: Some(environment_id),
                    ..Default::default()
                },
                range: golem_cli::model::local_observability::LocalMetricTimeRange {
                    from_unix_nanos: 0,
                    to_unix_nanos: 50,
                },
                name_filter: None,
                cursor: None,
                limit: Some(2),
            })
            .await
            .unwrap();
        assert_eq!(
            families
                .values
                .iter()
                .map(|family| family.name.as_str())
                .collect::<Vec<_>>(),
            vec!["cumulative", "delta"]
        );
        assert_eq!(families.next_cursor.as_deref(), Some("2"));
        let series = service
            .series(&LocalMetricSeriesRequest {
                source: LocalMetricSource::Agents,
                scope: LocalMetricScope {
                    environment_id: Some(environment_id),
                    ..Default::default()
                },
                range: golem_cli::model::local_observability::LocalMetricTimeRange {
                    from_unix_nanos: 0,
                    to_unix_nanos: 50,
                },
                family: "delta".to_string(),
                cursor: None,
                limit: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(series.values.len(), 1);
        assert_eq!(series.values[0].latest_value, Some(4.0));
        assert_eq!(series.values[0].latest_unix_nanos, Some(40));

        let delta = service
            .samples(&request("delta", &delta_series_key, 0, 50, 2))
            .await
            .unwrap();
        assert_eq!(
            delta
                .samples
                .iter()
                .map(|sample| sample.value)
                .collect::<Vec<_>>(),
            vec![3.0, 7.0]
        );
        assert!(delta.approximate);
        assert_eq!(delta.original_point_count, 4);

        let cumulative = service
            .samples(&request("cumulative", &cumulative_series_key, 10, 50, 2))
            .await
            .unwrap();
        assert_eq!(
            cumulative
                .samples
                .iter()
                .map(|sample| (sample.timestamp_unix_nanos, sample.value))
                .collect::<Vec<_>>(),
            vec![(10, 6.0), (30, 7.0)]
        );
        assert!(cumulative.approximate);

        let gauge = service
            .samples(&request("gauge", &gauge_series_key, 10, 50, 3))
            .await
            .unwrap();
        assert_eq!(
            gauge
                .samples
                .iter()
                .map(|sample| (sample.timestamp_unix_nanos, sample.value))
                .collect::<Vec<_>>(),
            vec![(10, 9.0), (20, 10.0), (40, 12.0)]
        );
        assert!(!gauge.approximate);
        assert_eq!(gauge.original_point_count, 2);
        assert_eq!(gauge.observed_from_unix_nanos, Some(20));
        assert_eq!(gauge.observed_to_unix_nanos, Some(40));

        drop(service);
        pool.close().await;
        clear_store(&db_path).await.unwrap();
        tokio::fs::remove_dir(&test_dir).await.unwrap();
    }

    #[test]
    async fn rejects_an_incompatible_store_with_clean_guidance() {
        let test_dir = std::env::temp_dir().join(format!(
            "golem-local-observability-old-schema-test-{}",
            Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&test_dir).await.unwrap();
        let db_path = test_dir.join(OBSERVABILITY_DB_FILE_NAME);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query("PRAGMA user_version = 1")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let error = open_store(&db_path).await.unwrap_err().to_string();

        assert!(error.contains("schema 1"), "{error}");
        assert!(
            error.contains("golem server clean --observability-only"),
            "{error}"
        );
        clear_store(&db_path).await.unwrap();
        tokio::fs::remove_dir(&test_dir).await.unwrap();
    }

    #[test]
    async fn private_routes_require_the_exact_bearer_token() {
        let service = Arc::new(LocalObservability {
            db_path: PathBuf::from("unused.db"),
            pool: None,
            state: RwLock::new(RuntimeState::Disabled),
            discarded_points: AtomicU64::new(0),
            last_service_sample: AtomicI64::new(0),
            last_agent_ingest: AtomicI64::new(0),
        });
        let app = add_routes(Route::new(), service, "expected-token").at(
            "*",
            poem::endpoint::make(|_: Request| async { StatusCode::NOT_FOUND }),
        );
        let client = TestClient::new(app);

        client
            .get(LOCAL_OBSERVABILITY_CAPABILITY_PATH)
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        client
            .get(LOCAL_OBSERVABILITY_CAPABILITY_PATH)
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        client
            .get(LOCAL_OBSERVABILITY_CAPABILITY_PATH)
            .header(header::AUTHORIZATION, "Bearer expected-token")
            .send()
            .await
            .assert_status_is_ok();
    }
}

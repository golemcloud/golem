// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use crate::config::McpImportResolverConfig;
use crate::services::account_usage::error::AccountUsageError;
use crate::services::mcp_oauth::{
    McpCredentialIdentity, McpImportAuth, McpOAuthError, McpOAuthService,
};
use futures::{StreamExt, future::join_all, stream};
use golem_common::SafeDisplay;
use golem_common::model::deployment::{DeployValidationWarning, McpImportDiscovery};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::{McpImportDeployment, McpImportSource};
use golem_common::model::tool::{RegisteredTool, ToolDeploymentState, ToolName};
use golem_mcp_import::tool::{Batch, Diagnostic, ProjectedTool, merge_imports, project_import};
use golem_mcp_import::transport::TransportError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::mcp_import::McpImportObservation;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;
use tokio::sync::{Semaphore, watch};

#[derive(Debug, Clone, thiserror::Error)]
pub enum McpImportResolverError {
    #[error("{}", .0.to_safe_string())]
    OAuth(Arc<McpOAuthError>),
    #[error("MCP import fetch failed: {0}")]
    Fetch(String),
    #[error("MCP import projection failed: {0}")]
    Projection(String),
    #[error("MCP import resolver timed out")]
    Timeout,
    #[error("MCP import source {import_index} is unavailable: {error}")]
    SourceUnavailable {
        import_index: usize,
        error: Box<Self>,
    },
}

impl From<McpOAuthError> for McpImportResolverError {
    fn from(value: McpOAuthError) -> Self {
        Self::OAuth(Arc::new(value))
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedImportedTool {
    pub source: McpImportSource,
    pub tool: ProjectedTool,
}

#[derive(Clone, Debug)]
pub struct ResolvedToolSet {
    pub native: Vec<RegisteredTool>,
    pub imported: Vec<ResolvedImportedTool>,
    pub diagnostics: Vec<(usize, Diagnostic)>,
}

#[derive(Clone, Debug)]
pub struct McpImportPreview {
    pub protocol_versions: Vec<String>,
    pub tools: Vec<(usize, ProjectedTool)>,
    pub diagnostics: Vec<(usize, Diagnostic)>,
    pub filtered: Vec<(usize, Diagnostic)>,
}

#[derive(Clone, Debug)]
pub enum ResolvedTool {
    Native(Box<RegisteredTool>),
    Imported(Box<ResolvedImportedTool>),
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct CacheKey {
    environment_id: EnvironmentId,
    deployment_revision: i64,
    import_index: u32,
    auth: McpImportAuth,
    credential: McpCredentialIdentity,
}

type Outcome = Result<Arc<McpImportObservation>, Arc<McpImportResolverError>>;

struct Entry {
    result: watch::Receiver<Option<Outcome>>,
    completed: Option<Instant>,
    last_used: Instant,
    success: Option<Arc<McpImportObservation>>,
    invalidated: bool,
}

struct Cache {
    entries: HashMap<CacheKey, Entry>,
    order: VecDeque<CacheKey>,
}

struct Inner {
    cache: Mutex<Cache>,
    permits: Arc<Semaphore>,
    config: McpImportResolverConfig,
}

pub struct McpImportResolver {
    oauth: Arc<McpOAuthService>,
    inner: Arc<Inner>,
}

impl McpImportResolver {
    pub async fn deployment_warnings(
        &self,
        environment_id: EnvironmentId,
        imports: Vec<McpImportDeployment>,
        native_names: Vec<String>,
        auth: AuthCtx,
    ) -> Vec<DeployValidationWarning> {
        if imports.is_empty() {
            return Vec::new();
        }
        match self
            .preview(environment_id, imports, native_names, auth)
            .await
        {
            Ok(preview) => preview
                .diagnostics
                .into_iter()
                .map(|(index, diagnostic)| {
                    DeployValidationWarning::McpImportDiscovery(McpImportDiscovery {
                        import_index: Some(index as u32),
                        upstream_tool_name: Some(diagnostic.upstream_name),
                        reason: diagnostic.reason,
                    })
                })
                .collect(),
            Err(error) => {
                let (import_index, error) = match error {
                    McpImportResolverError::SourceUnavailable {
                        import_index,
                        error,
                    } => (Some(import_index as u32), *error),
                    other => (None, other),
                };
                vec![DeployValidationWarning::McpImportDiscovery(
                    McpImportDiscovery {
                        import_index,
                        upstream_tool_name: None,
                        reason: format!(
                            "Deployment discovery unavailable; tools will be fetched on demand: {error}"
                        ),
                    },
                )]
            }
        }
    }

    pub async fn preview(
        &self,
        environment_id: EnvironmentId,
        imports: Vec<McpImportDeployment>,
        native_names: Vec<String>,
        auth: AuthCtx,
    ) -> Result<McpImportPreview, McpImportResolverError> {
        let deadline = tokio::time::Instant::now() + self.inner.config.operation_timeout;
        tokio::time::timeout_at(deadline, self.oauth.preview_policy(environment_id, &auth))
            .await
            .map_err(|_| McpImportResolverError::Timeout)??;
        if imports.len() > 128
            || native_names.len() > self.inner.config.projection.max_tools
            || serde_json::to_vec(&(&imports, &native_names))
                .map_err(|_| {
                    McpOAuthError::Transport(TransportError::InvalidInput(
                        "invalid MCP declarations".into(),
                    ))
                })?
                .len()
                > self.inner.config.transport.request_bytes
        {
            return Err(McpOAuthError::Transport(TransportError::InvalidInput(
                "MCP resolution request exceeds declaration limits".into(),
            ))
            .into());
        }
        let limits = self.inner.config.projection;
        let mut remaining_tools = limits.max_tools;
        let mut remaining_bytes = limits.max_listing_bytes;
        let mut observations = Vec::with_capacity(imports.len());
        for (index, import) in imports.into_iter().enumerate() {
            let result = tokio::time::timeout_at(deadline, async {
                let permit = self
                    .inner
                    .permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| McpImportResolverError::Fetch("resolver stopped".into()))?;
                let mut context = self
                    .oauth
                    .preview_context(environment_id, import, &auth, self.inner.config.transport)
                    .await
                    .map_err(McpImportResolverError::from)?;
                let listing = context
                    .client
                    .list_tools(&mut context.sender)
                    .await
                    .map_err(McpImportResolverError::from);
                if matches!(&listing, Err(error) if error.is_resource_unauthorized()) {
                    self.oauth
                        .expire_observed_credential(&context.identity)
                        .await?;
                }
                let listing = listing?;
                let tool_count = listing.tools.len();
                let (projected, listing_bytes) = super::run_cpu_bound_work(move || {
                    let _permit = permit;
                    if tool_count > remaining_tools {
                        return Err("MCP resolution tool count limit exceeded".to_string());
                    }
                    let projected = project_import(
                        &listing.tools,
                        context.import.prefix.as_deref(),
                        context.import.include.as_deref(),
                        context.import.exclude.as_deref(),
                        limits,
                    )?;
                    let bytes = serde_json::to_vec(&listing.tools)
                        .map_err(|_| "MCP listing cannot be serialized".to_string())?
                        .len();
                    if bytes > remaining_bytes {
                        return Err("MCP resolution listing byte limit exceeded".to_string());
                    }
                    Ok::<_, String>((projected, bytes))
                })
                .await
                .map_err(McpImportResolverError::Projection)?;
                Ok::<_, McpImportResolverError>((
                    listing.protocol_version,
                    projected,
                    tool_count,
                    listing_bytes,
                ))
            })
            .await
            .unwrap_or(Err(McpImportResolverError::Timeout))
            .map_err(|error| McpImportResolverError::SourceUnavailable {
                import_index: index,
                error: Box::new(error),
            })?;
            remaining_tools -= result.2;
            remaining_bytes -= result.3;
            observations.push((result.0, result.1));
        }
        let protocol_versions = observations
            .iter()
            .map(|(version, _)| version.clone())
            .collect();
        let filtered = observations
            .iter()
            .enumerate()
            .flat_map(|(index, (_, batch))| {
                batch
                    .filtered
                    .iter()
                    .cloned()
                    .map(move |diagnostic| (index, diagnostic))
            })
            .collect();
        let merged = merge_imports(
            &native_names.into_iter().collect(),
            observations.into_iter().map(|(_, batch)| batch).collect(),
        );
        Ok(McpImportPreview {
            protocol_versions,
            tools: merged.tools,
            diagnostics: merged.diagnostics,
            filtered,
        })
    }

    pub fn new(
        oauth: Arc<McpOAuthService>,
        config: McpImportResolverConfig,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        Ok(Self {
            oauth,
            inner: Arc::new(Inner {
                cache: Mutex::new(Cache {
                    entries: HashMap::new(),
                    order: VecDeque::new(),
                }),
                permits: Arc::new(Semaphore::new(config.fetch_concurrency)),
                config,
            }),
        })
    }

    pub async fn resolve_observation(
        &self,
        source: McpImportSource,
        auth: AuthCtx,
    ) -> Result<Arc<McpImportObservation>, McpImportResolverError> {
        self.resolve(
            source,
            McpImportAuth::Runtime(auth),
            false,
            true,
            None,
            tokio::time::Instant::now() + self.inner.config.operation_timeout,
        )
        .await
        .map(|(_, observation)| observation)
    }

    pub async fn inspect(
        &self,
        source: McpImportSource,
        auth: AuthCtx,
        refresh: bool,
    ) -> Result<Arc<McpImportObservation>, McpImportResolverError> {
        self.resolve(
            source,
            McpImportAuth::Operator(auth),
            refresh,
            true,
            None,
            tokio::time::Instant::now() + self.inner.config.operation_timeout,
        )
        .await
        .map(|(_, observation)| observation)
    }

    pub fn start_background_tasks(
        resolver: &Arc<Self>,
        join_set: &mut tokio::task::JoinSet<anyhow::Result<()>>,
    ) {
        let resolver = Arc::downgrade(resolver);
        join_set.spawn(async move {
            Self::run_refresh_loop(resolver).await;
            Ok(())
        });
    }

    async fn run_refresh_loop(resolver: Weak<Self>) {
        let Some(current) = resolver.upgrade() else {
            return;
        };
        let interval = current.inner.config.refresh_interval;
        drop(current);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Some(current) = resolver.upgrade() else {
                return;
            };
            current.refresh_active_entries().await;
            drop(current);
        }
    }

    async fn refresh_active_entries(&self) {
        let active: Vec<_> = {
            let cache = self.inner.cache.lock().unwrap();
            cache
                .entries
                .iter()
                .filter(|(_, entry)| {
                    !entry.invalidated
                        && entry.success.is_some()
                        && entry.last_used.elapsed() < self.inner.config.cache_ttl
                })
                .map(|(key, entry)| (key.clone(), entry.last_used))
                .collect()
        };
        let concurrency = (self.inner.config.fetch_concurrency / 2).max(1);
        stream::iter(active)
            .for_each_concurrent(concurrency, |(key, last_used)| async move {
                let source = McpImportSource {
                    environment_id: key.environment_id,
                    deployment_revision: key
                        .deployment_revision
                        .try_into()
                        .expect("cached revision"),
                    import_index: key.import_index,
                    upstream_tool_name: String::new(),
                };
                let result = self
                    .resolve(
                        source,
                        key.auth.clone(),
                        true,
                        false,
                        Some((key.clone(), last_used)),
                        tokio::time::Instant::now() + self.inner.config.operation_timeout,
                    )
                    .await;
                match &result {
                    Ok((credential, _)) => {
                        let mut cache = self.inner.cache.lock().unwrap();
                        if credential != &key.credential
                            && cache
                                .entries
                                .get(&key)
                                .is_some_and(|entry| entry.last_used <= last_used)
                        {
                            cache.entries.remove(&key);
                            cache.order.retain(|item| item != &key);
                        }
                    }
                    Err(error) if !error.allows_stale() => {
                        let mut cache = self.inner.cache.lock().unwrap();
                        if cache
                            .entries
                            .get(&key)
                            .is_some_and(|entry| entry.last_used <= last_used)
                        {
                            cache.entries.remove(&key);
                            cache.order.retain(|item| item != &key);
                        }
                    }
                    Err(error) => tracing::warn!(
                        environment_id = %key.environment_id,
                        deployment_revision = key.deployment_revision,
                        import_index = key.import_index,
                        error = %error,
                        "Periodic MCP import refresh failed; retaining prior observation"
                    ),
                }
            })
            .await;
    }

    /// Forces an upstream attempt. A failed attempt is returned while the last
    /// successful observation remains available to ordinary resolution.
    pub async fn refresh(
        &self,
        source: McpImportSource,
        auth: AuthCtx,
    ) -> Result<Arc<McpImportObservation>, McpImportResolverError> {
        self.resolve(
            source,
            McpImportAuth::Runtime(auth),
            true,
            true,
            None,
            tokio::time::Instant::now() + self.inner.config.operation_timeout,
        )
        .await
        .map(|(_, observation)| observation)
    }

    pub async fn report_resource_unauthorized(
        &self,
        source: &McpImportSource,
        auth: AuthCtx,
        used_generation: Option<uuid::Uuid>,
    ) -> Result<(), McpOAuthError> {
        let mut source = source.clone();
        source.upstream_tool_name.clear();
        self.oauth
            .report_resource_unauthorized(&source, auth.clone(), used_generation)
            .await?;

        let mut cache = self.inner.cache.lock().unwrap();
        let matching_keys: Vec<_> = cache
            .entries
            .keys()
            .filter(|key| {
                key.environment_id == source.environment_id
                    && key.deployment_revision == i64::from(source.deployment_revision)
                    && key.import_index == source.import_index
                    && key.auth == McpImportAuth::Runtime(auth.clone())
                    && match (&key.credential, used_generation) {
                        (McpCredentialIdentity::OAuth { generation, .. }, Some(used)) => {
                            *generation == used
                        }
                        (
                            McpCredentialIdentity::Anonymous | McpCredentialIdentity::Inline(_),
                            None,
                        ) => true,
                        _ => false,
                    }
            })
            .cloned()
            .collect();
        for key in matching_keys {
            if cache
                .entries
                .get(&key)
                .is_some_and(|entry| entry.completed.is_some())
            {
                cache.entries.remove(&key);
                cache.order.retain(|item| item != &key);
            } else if let Some(entry) = cache.entries.get_mut(&key) {
                entry.success = None;
                entry.invalidated = true;
            }
        }
        Ok(())
    }

    async fn resolve(
        &self,
        mut source: McpImportSource,
        auth: McpImportAuth,
        refresh: bool,
        record_demand: bool,
        background_origin: Option<(CacheKey, Instant)>,
        deadline: tokio::time::Instant,
    ) -> Result<(McpCredentialIdentity, Arc<McpImportObservation>), McpImportResolverError> {
        source.upstream_tool_name.clear();
        let context = tokio::time::timeout_at(deadline, async {
            let _permit = self
                .inner
                .permits
                .acquire()
                .await
                .map_err(|_| McpImportResolverError::Fetch("resolver stopped".into()))?;
            self.oauth
                .import_context(&source, auth.clone(), self.inner.config.transport)
                .await
                .map_err(McpImportResolverError::from)
        })
        .await
        .map_err(|_| McpImportResolverError::Timeout)??;
        let key = CacheKey {
            environment_id: source.environment_id,
            deployment_revision: source.deployment_revision.into(),
            import_index: source.import_index,
            auth,
            credential: context.identity.clone(),
        };
        let credential = key.credential.clone();
        let (mut receiver, previous) = self.receiver_or_start(
            key,
            source,
            context,
            refresh,
            record_demand,
            background_origin,
            deadline,
        );
        let outcome = tokio::time::timeout_at(deadline, async {
            loop {
                if let Some(value) = receiver.borrow().clone() {
                    break value;
                }
                receiver.changed().await.map_err(|_| {
                    Arc::new(McpImportResolverError::Fetch("cache fill stopped".into()))
                })?;
            }
        })
        .await
        .unwrap_or_else(|_| Err(Arc::new(McpImportResolverError::Timeout)));
        match outcome {
            Ok(value) => Ok((credential, value)),
            Err(error) if !refresh && error.allows_stale() => previous
                .map(|value| (credential, value))
                .ok_or_else(|| (*error).clone()),
            Err(error) => Err((*error).clone()),
        }
    }

    fn receiver_or_start(
        &self,
        key: CacheKey,
        source: McpImportSource,
        mut context: crate::services::mcp_oauth::McpRuntimeContext,
        refresh: bool,
        record_demand: bool,
        background_origin: Option<(CacheKey, Instant)>,
        deadline: tokio::time::Instant,
    ) -> (
        watch::Receiver<Option<Outcome>>,
        Option<Arc<McpImportObservation>>,
    ) {
        let mut cache = self.inner.cache.lock().unwrap();
        let inherited_last_used = if let Some((origin, _scanned_last_used)) = &background_origin {
            let Some(entry) = cache.entries.get(origin).filter(|entry| {
                !entry.invalidated
                    && entry.success.is_some()
                    && entry.last_used.elapsed() < self.inner.config.cache_ttl
            }) else {
                let (_, receiver) = watch::channel(Some(Err(Arc::new(
                    McpImportResolverError::Fetch("periodic refresh is obsolete".into()),
                ))));
                return (receiver, None);
            };
            Some(entry.last_used)
        } else {
            None
        };
        if record_demand && let Some(entry) = cache.entries.get_mut(&key) {
            entry.last_used = Instant::now();
        }
        if cache
            .entries
            .get(&key)
            .is_some_and(|entry| entry.completed.is_none() && entry.result.has_changed().is_err())
        {
            cache.entries.remove(&key);
            cache.order.retain(|item| item != &key);
        }
        let previous = cache
            .entries
            .get(&key)
            .filter(|entry| !entry.invalidated)
            .and_then(|entry| entry.success.clone());
        if !refresh {
            if let Some(entry) = cache.entries.get(&key) {
                let ttl = if entry.result.borrow().as_ref().is_some_and(Result::is_err) {
                    self.inner.config.failure_ttl
                } else {
                    self.inner.config.cache_ttl
                };
                let fresh = entry.completed.is_none_or(|at| at.elapsed() < ttl);
                if fresh && !entry.invalidated {
                    return (entry.result.clone(), previous);
                }
            }
        } else if let Some(entry) = cache.entries.get(&key)
            && entry.completed.is_none()
            && !entry.invalidated
        {
            return (entry.result.clone(), previous);
        }
        if cache.entries.len() >= self.inner.config.cache_entries
            && !cache.entries.contains_key(&key)
            && cache
                .entries
                .values()
                .all(|entry| entry.completed.is_none())
        {
            let (_, receiver) = watch::channel(Some(Err(Arc::new(McpImportResolverError::Fetch(
                "resolver cache is saturated".into(),
            )))));
            return (receiver, previous);
        }
        let (sender, receiver) = watch::channel(None);
        let existing_last_used = cache.entries.get(&key).map(|entry| entry.last_used);
        cache.entries.insert(
            key.clone(),
            Entry {
                result: receiver.clone(),
                completed: None,
                last_used: if record_demand {
                    Instant::now()
                } else {
                    existing_last_used
                        .into_iter()
                        .chain(inherited_last_used)
                        .max()
                        .unwrap_or_else(Instant::now)
                },
                success: previous.clone(),
                invalidated: false,
            },
        );
        cache.order.retain(|item| item != &key);
        cache.order.push_back(key.clone());
        while cache.entries.len() > self.inner.config.cache_entries {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            if cache
                .entries
                .get(&oldest)
                .is_some_and(|entry| entry.completed.is_none())
            {
                cache.order.push_back(oldest);
                if cache.order.iter().all(|item| {
                    cache
                        .entries
                        .get(item)
                        .is_some_and(|e| e.completed.is_none())
                }) {
                    break;
                }
            } else {
                cache.entries.remove(&oldest);
            }
        }
        drop(cache);
        let inner = self.inner.clone();
        let oauth = self.oauth.clone();
        let fetch_source = source.clone();
        let fetch_auth = key.auth.clone();
        let generation = match &key.credential {
            McpCredentialIdentity::OAuth { generation, .. } => Some(*generation),
            _ => None,
        };
        let fill_receiver = receiver.clone();
        tokio::spawn(async move {
            let result =
                tokio::time::timeout_at(deadline, async {
                    let permit =
                        inner.permits.clone().acquire_owned().await.map_err(|_| {
                            McpImportResolverError::Fetch("resolver stopped".into())
                        })?;
                    let listing = context
                        .client
                        .list_tools(&mut context.sender)
                        .await
                        .map_err(McpImportResolverError::from)?;
                    let limits = inner.config.projection;
                    let batch = super::run_cpu_bound_work(move || {
                        let _permit = permit;
                        project_import(
                            &listing.tools,
                            context.import.prefix.as_deref(),
                            context.import.include.as_deref(),
                            context.import.exclude.as_deref(),
                            limits,
                        )
                    })
                    .await
                    .map_err(McpImportResolverError::Projection)?;
                    Ok(Arc::new(McpImportObservation {
                        source,
                        protocol_version: listing.protocol_version,
                        tools: batch.tools,
                        diagnostics: batch.diagnostics,
                    }))
                })
                .await
                .unwrap_or(Err(McpImportResolverError::Timeout))
                .map_err(Arc::new);
            if matches!(&result, Err(error) if error.is_resource_unauthorized()) {
                // Do not retry a request here. Invalidate the used credential so
                // the next live resolution cannot reuse its metadata.
                let _ = tokio::time::timeout_at(
                    deadline,
                    oauth.report_import_unauthorized(&fetch_source, fetch_auth, generation),
                )
                .await;
            }
            let mut cache = inner.cache.lock().unwrap();
            if let Some(entry) = cache
                .entries
                .get_mut(&key)
                .filter(|entry| entry.result.same_channel(&fill_receiver))
            {
                entry.success = match &result {
                    Ok(value) => Some(value.clone()),
                    Err(error) if error.allows_stale() => entry.success.take(),
                    Err(_) => None,
                };
                entry.completed = Some(Instant::now());
            }
            // Publish after completing cache state: a new refresh must not see
            // the previous fill as pending once its waiters have returned.
            let _ = sender.send(Some(result));
        });
        (receiver, previous)
    }

    pub async fn list_tools(
        &self,
        environment_id: EnvironmentId,
        state: &ToolDeploymentState,
        auth: AuthCtx,
    ) -> Result<ResolvedToolSet, McpImportResolverError> {
        let deadline = tokio::time::Instant::now() + self.inner.config.operation_timeout;
        let native_names: BTreeSet<String> = state
            .registered_tools
            .keys()
            .map(|name| name.as_str().to_owned())
            .collect();
        let observations = join_all((0..state.mcp_imports.len()).map(|index| {
            let auth = auth.clone();
            async move {
                let source = McpImportSource {
                    environment_id,
                    deployment_revision: state.deployment_revision,
                    import_index: index as u32,
                    upstream_tool_name: String::new(),
                };
                self.resolve(
                    source,
                    McpImportAuth::Runtime(auth),
                    false,
                    true,
                    None,
                    deadline,
                )
                .await
                .map(|(_, observation)| observation)
                .map_err(|error| McpImportResolverError::SourceUnavailable {
                    import_index: index,
                    error: Box::new(error),
                })
            }
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let batches = observations
            .iter()
            .map(|observation| Batch {
                tools: observation.tools.clone(),
                diagnostics: observation.diagnostics.clone(),
                filtered: Vec::new(),
            })
            .collect();
        let merged = merge_imports(&native_names, batches);
        let imported = merged
            .tools
            .into_iter()
            .map(|(index, tool)| {
                let mut source = observations[index].source.clone();
                source.upstream_tool_name = tool.upstream_name.clone();
                ResolvedImportedTool { source, tool }
            })
            .collect();
        Ok(ResolvedToolSet {
            native: state.registered_tools.values().cloned().collect(),
            imported,
            diagnostics: merged.diagnostics,
        })
    }

    pub async fn lookup_tool(
        &self,
        environment_id: EnvironmentId,
        state: &ToolDeploymentState,
        name: &ToolName,
        auth: AuthCtx,
    ) -> Result<Option<ResolvedTool>, McpImportResolverError> {
        if let Some(native) = state.registered_tools.get(name) {
            return Ok(Some(ResolvedTool::Native(Box::new(native.clone()))));
        }
        let deadline = tokio::time::Instant::now() + self.inner.config.operation_timeout;
        for index in 0..state.mcp_imports.len() {
            let source = McpImportSource {
                environment_id,
                deployment_revision: state.deployment_revision,
                import_index: index as u32,
                upstream_tool_name: String::new(),
            };
            let observation = self
                .resolve(
                    source,
                    McpImportAuth::Runtime(auth.clone()),
                    false,
                    true,
                    None,
                    deadline,
                )
                .await
                .map(|(_, observation)| observation)
                .map_err(|error| McpImportResolverError::SourceUnavailable {
                    import_index: index,
                    error: Box::new(error),
                })?;
            if let Some(tool) = observation
                .tools
                .iter()
                .find(|tool| tool.definition.name() == Some(name.as_str()))
            {
                let mut source = observation.source.clone();
                source.upstream_tool_name = tool.upstream_name.clone();
                return Ok(Some(ResolvedTool::Imported(Box::new(
                    ResolvedImportedTool {
                        source,
                        tool: tool.clone(),
                    },
                ))));
            }
        }
        Ok(None)
    }
}

impl McpImportResolverError {
    fn is_resource_unauthorized(&self) -> bool {
        matches!(self, Self::OAuth(error) if matches!(error.as_ref(),
            McpOAuthError::Transport(TransportError::AuthorizationRequired(401))))
    }

    fn allows_stale(&self) -> bool {
        match self {
            Self::OAuth(error) => matches!(
                error.as_ref(),
                McpOAuthError::Transport(
                    TransportError::Network
                        | TransportError::Timeout
                        | TransportError::HttpStatus(_)
                        | TransportError::Protocol(_)
                        | TransportError::Limit(_)
                        | TransportError::Remote { .. }
                        | TransportError::UnsupportedCapability
                ) | McpOAuthError::InternalError(_)
                    | McpOAuthError::AccountUsage(AccountUsageError::InternalError(_))
            ),
            Self::Fetch(_) | Self::Timeout | Self::Projection(_) => true,
            Self::SourceUnavailable { error, .. } => error.allows_stale(),
        }
    }
}

#[cfg(test)]
mod tests;

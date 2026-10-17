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

use golem_common::SafeDisplay;
use golem_common::config::RedisConfig;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::RetryConfig;
use golem_common::model::base64::Base64;
use golem_common::model::invocation_session_public::MAX_WEBSOCKET_MESSAGE_SIZE;
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::clients::shard_manager::GrpcShardManagerConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::http_server::HttpServerConfig;
use golem_service_base::service::routing_table::RoutingTableConfig;
use poem::web::websocket::WebSocketConfig;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceConfig {
    pub environment: String,
    pub tracing: TracingConfig,
    pub gateway_session_storage: SessionStoreConfig,
    pub blob_storage: BlobStorageConfig,
    /// The address every listener of this service binds to. `0.0.0.0` accepts IPv4 only; `::`
    /// accepts both IPv6 and IPv4.
    pub bind_address: IpAddr,
    pub port: u16,
    #[serde(with = "humantime_serde")]
    pub runtime_metrics_sampling_interval: Duration,
    pub custom_request_port: u16,
    pub grpc: GrpcApiConfig,
    pub shard_manager: GrpcShardManagerConfig,
    pub routing_table: RoutingTableConfig,
    pub worker_executor: WorkerExecutorClientConfig,
    pub workspace: String,
    pub registry_service: GrpcRegistryServiceConfig,
    pub mcp_port: u16,
    pub cors_origin_regex: String,
    pub route_resolver: RouteResolverConfig,
    pub component_service: ComponentServiceConfig,
    pub auth_service: AuthServiceConfig,
    pub webhook_callback_handler: WebhookCallbackHandlerConfig,
    pub invocation_session_tokens: InvocationSessionTokenConfig,
    #[serde(default)]
    pub agent_resolution_cache: AgentResolutionCacheConfig,
    #[serde(default)]
    pub http_session: HttpSessionLimits,
    #[serde(default)]
    pub durable_streams: DurableStreamsConfig,
    /// HTTP/2 settings of the API, custom request and MCP ports.
    #[serde(default)]
    pub http_server: HttpServerConfig,
    #[serde(default)]
    pub body_limits: RequestBodyLimits,
    #[serde(default)]
    pub websocket_limits: WebSocketLimits,
}

impl WorkerServiceConfig {
    pub fn validate_http_limits(&self) -> Result<(), String> {
        self.http_server.validate()?;
        self.websocket_limits.validate()
    }

    pub fn socket_addr(&self, port: u16) -> SocketAddr {
        SocketAddr::new(self.bind_address, port)
    }

    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl SafeDisplay for WorkerServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "gateway session storage:");
        let _ = writeln!(
            result,
            "{}",
            self.gateway_session_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "bind address: {}", self.bind_address);
        let _ = writeln!(&mut result, "HTTP port: {}", self.port);
        let _ = writeln!(
            &mut result,
            "runtime metrics sampling interval: {}s",
            self.runtime_metrics_sampling_interval.as_secs()
        );
        let _ = writeln!(
            &mut result,
            "Custom request port: {}",
            self.custom_request_port
        );
        let _ = writeln!(&mut result, "MCP port: {}", self.mcp_port);

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "shard manager:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.shard_manager.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "routing table:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.routing_table.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "worker executor:");
        let _ = writeln!(result, "{}", self.worker_executor.to_safe_string_indented());

        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "registry service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.registry_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        let _ = writeln!(&mut result, "route resolver:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.route_resolver.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "auth service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.auth_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "webhook callback handler:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.webhook_callback_handler.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "invocation session tokens:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.invocation_session_tokens.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "agent resolution cache:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_resolution_cache.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "HTTP session:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.http_session.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "durable streams:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.durable_streams.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "HTTP server:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.http_server.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "request body limits: {:?}", self.body_limits);
        let _ = writeln!(&mut result, "WebSocket limits: {:?}", self.websocket_limits);

        result
    }
}

impl Default for WorkerServiceConfig {
    fn default() -> Self {
        Self {
            environment: "local".to_string(),
            gateway_session_storage: SessionStoreConfig::Redis(Default::default()),
            blob_storage: BlobStorageConfig::default(),
            tracing: TracingConfig::local_dev("worker-service"),
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 9005,
            runtime_metrics_sampling_interval: Duration::from_secs(5),
            custom_request_port: 9006,
            mcp_port: 9007,
            grpc: GrpcApiConfig::default(),
            shard_manager: GrpcShardManagerConfig::default(),
            routing_table: RoutingTableConfig::default(),
            worker_executor: WorkerExecutorClientConfig::default(),
            workspace: "release".to_string(),
            registry_service: GrpcRegistryServiceConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            route_resolver: RouteResolverConfig::default(),
            component_service: ComponentServiceConfig::default(),
            auth_service: AuthServiceConfig::default(),
            webhook_callback_handler: WebhookCallbackHandlerConfig::default(),
            invocation_session_tokens: InvocationSessionTokenConfig::default(),
            agent_resolution_cache: AgentResolutionCacheConfig::default(),
            http_session: HttpSessionLimits::default(),
            durable_streams: DurableStreamsConfig::default(),
            http_server: HttpServerConfig::default(),
            body_limits: RequestBodyLimits::default(),
            websocket_limits: WebSocketLimits::default(),
        }
    }
}

impl HasConfigExamples<WorkerServiceConfig> for WorkerServiceConfig {
    fn examples() -> Vec<ConfigExample<WorkerServiceConfig>> {
        vec![]
    }
}

/// The payload limit of every path into an agent: the gRPC message to the worker executor, the
/// invocation session message, and the REST and MCP request body.
const AGENT_PAYLOAD_LIMIT: usize = 32 * 1024 * 1024;

/// Largest request bodies, in bytes. A larger body gets `413 Payload Too Large`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestBodyLimits {
    /// The REST API on the API port.
    pub api: usize,
    /// MCP deployments, on the MCP port and the custom request port.
    pub mcp: usize,
}

impl Default for RequestBodyLimits {
    fn default() -> Self {
        Self {
            api: AGENT_PAYLOAD_LIMIT,
            mcp: AGENT_PAYLOAD_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSocketLimits {
    /// The invocation session (`/v1/agents/invoke-agent-session`). Its message and frame
    /// limits must be above the 32 MiB message limit of the session protocol, so that the
    /// session itself rejects a larger message with close code 1009.
    pub invocation_session: WebSocketTransportLimits,
    /// The agent event stream (`/v1/components/:component_id/workers/:agent_name/connect`).
    pub connect: WebSocketTransportLimits,
}

impl WebSocketLimits {
    pub fn validate(&self) -> Result<(), String> {
        self.invocation_session.validate("invocation_session")?;
        self.connect.validate("connect")?;
        let session = &self.invocation_session;
        if session.max_message_size <= MAX_WEBSOCKET_MESSAGE_SIZE
            || session.max_frame_size <= MAX_WEBSOCKET_MESSAGE_SIZE
        {
            return Err(format!(
                "websocket_limits.invocation_session.max_message_size and max_frame_size must be above the session message limit of {MAX_WEBSOCKET_MESSAGE_SIZE} bytes"
            ));
        }
        Ok(())
    }
}

/// Limits, in bytes, of a WebSocket connection. An inbound message or frame above its limit
/// ends the connection without a close frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSocketTransportLimits {
    pub max_message_size: usize,
    pub max_frame_size: usize,
    /// Outbound bytes that may wait for the client before a send fails.
    pub max_write_buffer_size: usize,
}

impl WebSocketTransportLimits {
    pub fn websocket_config(&self) -> WebSocketConfig {
        WebSocketConfig::default()
            .max_message_size(Some(self.max_message_size))
            .max_frame_size(Some(self.max_frame_size))
            .max_write_buffer_size(self.max_write_buffer_size)
    }

    fn validate(&self, name: &str) -> Result<(), String> {
        let write_buffer_size = WebSocketConfig::default().write_buffer_size;
        if self.max_write_buffer_size <= write_buffer_size {
            return Err(format!(
                "websocket_limits.{name}.max_write_buffer_size must be above the write buffer size of {write_buffer_size} bytes"
            ));
        }
        Ok(())
    }
}

impl Default for WebSocketTransportLimits {
    fn default() -> Self {
        // A little above the payload limit, so that the invocation session receives an
        // oversized message and answers it with close code 1009.
        let message_limit = AGENT_PAYLOAD_LIMIT + 64 * 1024;
        Self {
            max_message_size: message_limit,
            max_frame_size: message_limit,
            max_write_buffer_size: 2 * AGENT_PAYLOAD_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpSessionLimits {
    /// Encoded input credit budget, including per-byte protobuf nodes and frame overhead.
    /// This is not a raw HTTP body-size limit; credits are returned after executor ACKs.
    pub retained_input_bytes: usize,
    pub retained_input_frames: usize,
    /// Maximum queued response frames. Together with `max_event_bytes`, this bounds queued output
    /// allocation to `event_queue_frames * max_event_bytes` bytes.
    pub event_queue_frames: usize,
    pub command_queue_frames: usize,
    /// Lifetime reattachment budget for one exchange, including successful reattachments.
    pub max_resume_attempts: usize,
    #[serde(with = "humantime_serde")]
    pub retry_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub retry_deadline: Duration,
    #[serde(with = "humantime_serde")]
    pub cleanup_timeout: Duration,
    /// Hard duration cap on the whole HTTP exchange, including streaming response delivery.
    #[serde(with = "humantime_serde")]
    pub exchange_timeout: Duration,
    /// Maximum encoded event size, including the start response head and every subsequent frame.
    pub max_event_bytes: usize,
}

impl HttpSessionLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.retained_input_bytes < 136 {
            return Err("retained_input_bytes must be at least 136".into());
        }
        if self.retained_input_bytes > u32::MAX as usize {
            return Err("retained_input_bytes must fit in u32".into());
        }
        if self.retained_input_frames == 0
            || self.event_queue_frames == 0
            || self.command_queue_frames == 0
            || self.max_resume_attempts == 0
            || self.retry_deadline.is_zero()
            || self.cleanup_timeout.is_zero()
            || self.exchange_timeout.is_zero()
            || self.max_event_bytes == 0
        {
            return Err("HTTP session limits and durations must be non-zero".into());
        }
        let input_channel_capacity = self
            .retained_input_frames
            .checked_add(2)
            .ok_or("retained input channel capacity overflows")?;
        for (name, frames) in [
            ("retained_input_frames", self.retained_input_frames),
            ("event_queue_frames", self.event_queue_frames),
            ("command_queue_frames", self.command_queue_frames),
        ] {
            if frames > tokio::sync::Semaphore::MAX_PERMITS {
                return Err(format!("{name} exceeds the maximum supported capacity"));
            }
        }
        if input_channel_capacity > tokio::sync::Semaphore::MAX_PERMITS {
            return Err("retained input channel exceeds the maximum supported capacity".into());
        }
        self.event_queue_frames
            .checked_mul(self.max_event_bytes)
            .ok_or("queued output byte budget overflows")?;
        Ok(())
    }
}

impl Default for HttpSessionLimits {
    fn default() -> Self {
        Self {
            retained_input_bytes: 4 * 1024 * 1024,
            retained_input_frames: 32,
            event_queue_frames: 2,
            command_queue_frames: 16,
            max_resume_attempts: 4,
            retry_delay: Duration::from_millis(100),
            retry_deadline: Duration::from_secs(10),
            cleanup_timeout: Duration::from_secs(2),
            exchange_timeout: Duration::from_secs(300),
            // The executor permits a 16 MiB encoded item; allow its transport envelope too.
            max_event_bytes: 17 * 1024 * 1024,
        }
    }
}

impl SafeDisplay for HttpSessionLimits {
    fn to_safe_string(&self) -> String {
        format!("{self:#?}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
    #[serde(default = "default_grpc_max_message_size")]
    pub max_message_size: usize,
}

fn default_grpc_max_message_size() -> usize {
    32 * 1024 * 1024
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9094,
            tls: GrpcServerTlsConfig::disabled(),
            max_message_size: default_grpc_max_message_size(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum SessionStoreConfig {
    Redis(RedisSessionStoreConfig),
    Sqlite(SqliteSessionStoreConfig),
    Postgres(PostgresSessionStoreConfig),
}

impl SafeDisplay for SessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            SessionStoreConfig::Redis(redis) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", redis.to_safe_string_indented());
            }
            SessionStoreConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", sqlite.to_safe_string_indented());
            }
            SessionStoreConfig::Postgres(postgres) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", postgres.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedisSessionStoreConfig {
    #[serde(with = "humantime_serde")]
    pub pending_login_expiration: std::time::Duration,
    #[serde(flatten)]
    pub redis_config: RedisConfig,
}

impl Default for RedisSessionStoreConfig {
    fn default() -> Self {
        Self {
            pending_login_expiration: Duration::from_hours(1),
            redis_config: RedisConfig::default(),
        }
    }
}

impl SafeDisplay for RedisSessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "pending_login_expiration: {:?}",
            self.pending_login_expiration
        );
        let _ = writeln!(&mut result, "{}", self.redis_config.to_safe_string());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqliteSessionStoreConfig {
    #[serde(with = "humantime_serde")]
    pub pending_login_expiration: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: std::time::Duration,
    #[serde(flatten)]
    pub sqlite_config: DbSqliteConfig,
}

impl SafeDisplay for SqliteSessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "pending_login_expiration: {:?}",
            self.pending_login_expiration
        );
        let _ = writeln!(&mut result, "cleanup_interval: {:?}", self.cleanup_interval);
        let _ = writeln!(&mut result, "{}", self.sqlite_config.to_safe_string());
        result
    }
}

/// Schema of the Postgres session store when `schema` is not set.
///
/// Every Golem service applies its own migrations, and sqlx keeps the applied versions in a
/// `_sqlx_migrations` table in the current schema. Defaulting to `public` would make the session
/// store collide with any other service that uses `public` in the same database.
pub const DEFAULT_POSTGRES_SESSION_STORE_SCHEMA: &str = "golem_worker_service_session_store";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostgresSessionStoreConfig {
    #[serde(with = "humantime_serde")]
    pub pending_login_expiration: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: std::time::Duration,
    /// When `schema` is not set, the store uses [`DEFAULT_POSTGRES_SESSION_STORE_SCHEMA`].
    #[serde(flatten)]
    pub postgres_config: DbPostgresConfig,
}

impl PostgresSessionStoreConfig {
    pub fn postgres_config_with_schema(&self) -> DbPostgresConfig {
        let mut postgres_config = self.postgres_config.clone();
        postgres_config
            .schema
            .get_or_insert_with(|| DEFAULT_POSTGRES_SESSION_STORE_SCHEMA.to_string());
        postgres_config
    }
}

impl SafeDisplay for PostgresSessionStoreConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "pending_login_expiration: {:?}",
            self.pending_login_expiration
        );
        let _ = writeln!(&mut result, "cleanup_interval: {:?}", self.cleanup_interval);
        let _ = writeln!(
            &mut result,
            "{}",
            self.postgres_config_with_schema().to_safe_string()
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteResolverConfig {
    pub router_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub router_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub router_cache_eviction_period: Duration,
    /// Direct peer addresses whose X-Forwarded-Proto and X-Forwarded-Host are authoritative.
    #[serde(default)]
    pub trusted_ingress_addresses: Vec<IpAddr>,
}

impl SafeDisplay for RouteResolverConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "router_cache_max_capacity: {}",
            self.router_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "trusted_ingress_addresses: {:?}",
            self.trusted_ingress_addresses
        );
        let _ = writeln!(&mut result, "router_cache_ttl: {:?}", self.router_cache_ttl);
        let _ = writeln!(
            &mut result,
            "router_cache_eviction_period: {:?}",
            self.router_cache_eviction_period
        );
        result
    }
}

impl Default for RouteResolverConfig {
    fn default() -> Self {
        Self {
            router_cache_max_capacity: 1024,
            router_cache_ttl: Duration::from_mins(10),
            router_cache_eviction_period: Duration::from_mins(1),
            trusted_ingress_addresses: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentResolutionCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub eviction_period: Duration,
}

impl SafeDisplay for AgentResolutionCacheConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max_capacity: {}", self.max_capacity);
        let _ = writeln!(&mut result, "ttl: {:?}", self.ttl);
        let _ = writeln!(&mut result, "eviction_period: {:?}", self.eviction_period);
        result
    }
}

impl Default for AgentResolutionCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 10_000,
            ttl: Duration::from_secs(3600),
            eviction_period: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableStreamsConfig {
    #[serde(with = "humantime_serde")]
    pub long_poll_timeout: Duration,
    pub max_append_body_bytes: usize,
    pub forks: DurableStreamsForksConfig,
    pub load: DurableStreamsLoadConfig,
}

impl SafeDisplay for DurableStreamsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "long_poll_timeout: {:?}",
            self.long_poll_timeout
        );
        let _ = writeln!(
            &mut result,
            "max_append_body_bytes: {}",
            self.max_append_body_bytes
        );
        let _ = writeln!(&mut result, "forks:");
        let _ = writeln!(&mut result, "{}", self.forks.to_safe_string_indented());
        let _ = writeln!(&mut result, "load:");
        let _ = writeln!(&mut result, "{}", self.load.to_safe_string_indented());
        result
    }
}

impl Default for DurableStreamsConfig {
    fn default() -> Self {
        Self {
            long_poll_timeout: Duration::from_secs(30),
            max_append_body_bytes: 1024 * 1024,
            forks: DurableStreamsForksConfig::default(),
            load: DurableStreamsLoadConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableStreamsForksConfig {
    pub max_forks_per_session: u32,
    pub max_forks_per_second: u32,
    pub max_copied_bytes: u64,
}

impl SafeDisplay for DurableStreamsForksConfig {
    fn to_safe_string(&self) -> String {
        format!(
            "max_forks_per_session: {}\nmax_forks_per_second: {}\nmax_copied_bytes: {}",
            self.max_forks_per_session, self.max_forks_per_second, self.max_copied_bytes
        )
    }
}

impl Default for DurableStreamsForksConfig {
    fn default() -> Self {
        Self {
            max_forks_per_session: 128,
            max_forks_per_second: 4,
            max_copied_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableStreamsLoadConfig {
    pub max_concurrent_readers_per_stream: usize,
    pub max_concurrent_readers_per_node: usize,
    pub max_catch_up_requests_per_second_per_stream: u32,
    pub max_append_requests_per_second_per_stream: u32,
}

impl SafeDisplay for DurableStreamsLoadConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "max_concurrent_readers_per_stream: {}",
            self.max_concurrent_readers_per_stream
        );
        let _ = writeln!(
            &mut result,
            "max_concurrent_readers_per_node: {}",
            self.max_concurrent_readers_per_node
        );
        let _ = writeln!(
            &mut result,
            "max_catch_up_requests_per_second_per_stream: {}",
            self.max_catch_up_requests_per_second_per_stream
        );
        let _ = writeln!(
            &mut result,
            "max_append_requests_per_second_per_stream: {}",
            self.max_append_requests_per_second_per_stream
        );
        result
    }
}

impl Default for DurableStreamsLoadConfig {
    fn default() -> Self {
        Self {
            max_concurrent_readers_per_stream: 16,
            max_concurrent_readers_per_node: 4096,
            max_catch_up_requests_per_second_per_stream: 100,
            max_append_requests_per_second_per_stream: 100,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub component_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub current_component_cache_ttl: Duration,
}

impl SafeDisplay for ComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "component_cache_max_capacity: {}",
            self.component_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "current_component_cache_ttl: {:?}",
            self.current_component_cache_ttl
        );
        result
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            component_cache_max_capacity: 1024,
            current_component_cache_ttl: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthServiceConfig {
    pub auth_ctx_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub auth_ctx_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub auth_ctx_cache_eviction_period: Duration,

    pub environment_auth_details_cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub environment_auth_details_cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub environment_auth_details_cache_eviction_period: Duration,
}

impl SafeDisplay for AuthServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_max_capacity: {}",
            self.auth_ctx_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_ttl: {:?}",
            self.auth_ctx_cache_ttl
        );
        let _ = writeln!(
            &mut result,
            "auth_ctx_cache_eviction_period: {:?}",
            self.auth_ctx_cache_eviction_period
        );

        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_max_capacity: {}",
            self.environment_auth_details_cache_max_capacity
        );
        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_ttl: {:?}",
            self.environment_auth_details_cache_ttl
        );
        let _ = writeln!(
            &mut result,
            "environment_auth_details_cache_eviction_period: {:?}",
            self.environment_auth_details_cache_eviction_period
        );

        result
    }
}

impl Default for AuthServiceConfig {
    fn default() -> Self {
        Self {
            auth_ctx_cache_max_capacity: 1024,
            auth_ctx_cache_ttl: Duration::from_mins(10),
            auth_ctx_cache_eviction_period: Duration::from_mins(1),

            environment_auth_details_cache_max_capacity: 1024,
            environment_auth_details_cache_ttl: Duration::from_mins(10),
            environment_auth_details_cache_eviction_period: Duration::from_mins(1),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorClientConfig {
    pub retries: RetryConfig,
    /// Maximum number of oplog entry reads that hold a full entry in memory at the same time.
    /// Further reads wait for a running read to end.
    pub max_concurrent_oplog_entry_reads: usize,
    #[serde(flatten)]
    pub client: GrpcClientConfig,
}

impl Default for WorkerExecutorClientConfig {
    fn default() -> Self {
        Self {
            retries: RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_secs(3),
                multiplier: 10.0,
                max_jitter_factor: Some(0.15),
            },
            max_concurrent_oplog_entry_reads: 4,
            client: GrpcClientConfig {
                retries_on_unavailable: RetryConfig {
                    max_attempts: 0, // we want to invalidate the routing table asap
                    min_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(2),
                    multiplier: 2.0,
                    max_jitter_factor: Some(0.15),
                },
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

impl WorkerExecutorClientConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_concurrent_oplog_entry_reads == 0
            || self.max_concurrent_oplog_entry_reads > tokio::sync::Semaphore::MAX_PERMITS
        {
            return Err(format!(
                "worker_executor.max_concurrent_oplog_entry_reads must be between 1 and {}",
                tokio::sync::Semaphore::MAX_PERMITS
            ));
        }
        Ok(())
    }
}

impl SafeDisplay for WorkerExecutorClientConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(result, "{}", self.retries.to_safe_string_indented());
        let _ = writeln!(
            &mut result,
            "max concurrent oplog entry reads: {}",
            self.max_concurrent_oplog_entry_reads
        );

        let _ = writeln!(&mut result, "{}", self.client.to_safe_string());

        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookCallbackHandlerConfig {
    pub hmac_key: Base64,
}

impl SafeDisplay for WebhookCallbackHandlerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "hmac_key: *******");
        result
    }
}

impl Default for WebhookCallbackHandlerConfig {
    fn default() -> Self {
        Self {
            hmac_key: Base64(vec![
                0x2b, 0x7e, 0x02, 0xa3, 0x8a, 0x51, 0x30, 0x39, 0x7b, 0x74, 0x1d, 0xdc, 0x60, 0x1f,
                0xb5, 0xfc, 0xdd, 0x09, 0xde, 0xd3, 0x33, 0x25, 0x62, 0x38, 0x17, 0x23, 0xcd, 0x3a,
                0xc9, 0x86, 0x1e, 0x41,
            ]),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvocationSessionTokenConfig {
    pub issuer: String,
    pub active_key: InvocationSessionTokenKeyConfig,
    #[serde(default)]
    pub verify_only_keys: Vec<InvocationSessionTokenKeyConfig>,
}

impl SafeDisplay for InvocationSessionTokenConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "issuer: {}", self.issuer);
        let _ = writeln!(&mut result, "active_key_id: {}", self.active_key.id);
        let verify_only_ids = self
            .verify_only_keys
            .iter()
            .map(|key| key.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(&mut result, "verify_only_key_ids: [{verify_only_ids}]");
        result
    }
}

impl Default for InvocationSessionTokenConfig {
    fn default() -> Self {
        Self {
            issuer: "local-worker-service".to_string(),
            active_key: InvocationSessionTokenKeyConfig {
                id: "local-development".to_string(),
                key: Base64(vec![
                    0xd2, 0xe7, 0x40, 0x2d, 0xe4, 0x91, 0x72, 0x29, 0x81, 0x9a, 0x62, 0xd7, 0x9b,
                    0xa8, 0x8a, 0x63, 0x58, 0xe0, 0xe3, 0xe2, 0x19, 0x35, 0x7b, 0xfa, 0x2d, 0xa1,
                    0x64, 0x74, 0x84, 0x38, 0x84, 0xb0,
                ]),
            },
            verify_only_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvocationSessionTokenKeyConfig {
    pub id: String,
    pub key: Base64,
}

const CONFIG_FILE_NAME: &str = "config/worker-service.toml";

pub fn make_worker_service_config_loader() -> ConfigLoader<WorkerServiceConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from(CONFIG_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use test_r::test;

    use super::{
        DEFAULT_POSTGRES_SESSION_STORE_SCHEMA, HttpSessionLimits, SessionStoreConfig,
        WorkerServiceConfig, make_worker_service_config_loader,
    };
    use figment::providers::{Format, Toml};
    use golem_common::SafeDisplay;

    #[test]
    pub fn config_is_loadable() {
        make_worker_service_config_loader()
            .load_or_dump_config()
            .expect("Failed to load config");
    }

    #[test]
    fn postgres_session_store_overrides_the_default_redis_session_store() {
        let config: WorkerServiceConfig = make_worker_service_config_loader()
            .figment()
            .merge(Toml::string(
                r#"
                [gateway_session_storage]
                type = "Postgres"

                [gateway_session_storage.config]
                pending_login_expiration = "30m"
                cleanup_interval = "1m"
                host = "postgres.internal"
                port = 5433
                database = "golem"
                username = "golem-user"
                password = "golem-password"
                max_connections = 4
                "#,
            ))
            .extract()
            .expect("Failed to load config");

        let SessionStoreConfig::Postgres(postgres) = &config.gateway_session_storage else {
            panic!("expected a Postgres session store config");
        };
        assert_eq!(
            postgres.pending_login_expiration,
            Duration::from_secs(30 * 60)
        );
        assert_eq!(postgres.cleanup_interval, Duration::from_secs(60));
        assert_eq!(postgres.postgres_config.host, "postgres.internal");
        assert_eq!(postgres.postgres_config.port, 5433);
        assert_eq!(postgres.postgres_config.database, "golem");
        assert_eq!(postgres.postgres_config.max_connections, 4);
        assert_eq!(postgres.postgres_config.schema, None);
        assert_eq!(
            postgres.postgres_config_with_schema().schema.as_deref(),
            Some(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA)
        );

        let displayed = config.gateway_session_storage.to_safe_string();
        assert!(displayed.contains("postgres:"));
        assert!(displayed.contains(&format!("schema: {DEFAULT_POSTGRES_SESSION_STORE_SCHEMA}")));
        assert!(!displayed.contains("golem-user"));
        assert!(!displayed.contains("golem-password"));
    }

    #[test]
    fn postgres_session_store_keeps_an_explicit_schema() {
        let config: WorkerServiceConfig = make_worker_service_config_loader()
            .figment()
            .merge(Toml::string(
                r#"
                [gateway_session_storage]
                type = "Postgres"

                [gateway_session_storage.config]
                pending_login_expiration = "1h"
                cleanup_interval = "5m"
                host = "localhost"
                port = 5432
                database = "golem"
                username = "postgres"
                password = "postgres"
                max_connections = 10
                schema = "worker_service_sessions"
                "#,
            ))
            .extract()
            .expect("Failed to load config");

        let SessionStoreConfig::Postgres(postgres) = &config.gateway_session_storage else {
            panic!("expected a Postgres session store config");
        };
        assert_eq!(
            postgres.postgres_config_with_schema().schema.as_deref(),
            Some("worker_service_sessions")
        );
    }

    #[test]
    fn default_http_session_limits_and_trusted_ingress_are_safe() {
        let config = WorkerServiceConfig::default();

        assert!(config.http_session.validate().is_ok());
        assert!(config.route_resolver.trusted_ingress_addresses.is_empty());
    }

    #[test]
    fn http_session_limits_reject_zero_counts_and_sizes() {
        let invalid = [
            HttpSessionLimits {
                retained_input_frames: 0,
                ..Default::default()
            },
            HttpSessionLimits {
                event_queue_frames: 0,
                ..Default::default()
            },
            HttpSessionLimits {
                command_queue_frames: 0,
                ..Default::default()
            },
            HttpSessionLimits {
                max_resume_attempts: 0,
                ..Default::default()
            },
            HttpSessionLimits {
                max_event_bytes: 0,
                ..Default::default()
            },
        ];

        for limits in invalid {
            assert!(limits.validate().is_err());
        }
    }

    #[test]
    fn http_session_limits_reject_invalid_input_byte_bounds() {
        assert!(
            HttpSessionLimits {
                retained_input_bytes: 135,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            HttpSessionLimits {
                retained_input_bytes: u32::MAX as usize + 1,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn http_session_limits_reject_capacity_and_budget_overflow() {
        let invalid = [
            HttpSessionLimits {
                retained_input_frames: usize::MAX,
                ..Default::default()
            },
            HttpSessionLimits {
                event_queue_frames: tokio::sync::Semaphore::MAX_PERMITS + 1,
                ..Default::default()
            },
            HttpSessionLimits {
                command_queue_frames: tokio::sync::Semaphore::MAX_PERMITS + 1,
                ..Default::default()
            },
            HttpSessionLimits {
                event_queue_frames: 2,
                max_event_bytes: usize::MAX,
                ..Default::default()
            },
        ];

        for limits in invalid {
            assert!(limits.validate().is_err());
        }
    }

    #[test]
    fn http_session_limits_reject_zero_required_timeouts() {
        let invalid = [
            HttpSessionLimits {
                retry_deadline: Duration::ZERO,
                ..Default::default()
            },
            HttpSessionLimits {
                cleanup_timeout: Duration::ZERO,
                ..Default::default()
            },
            HttpSessionLimits {
                exchange_timeout: Duration::ZERO,
                ..Default::default()
            },
        ];

        for limits in invalid {
            assert!(limits.validate().is_err());
        }
    }

    #[test]
    fn http_limits_default_to_the_agent_payload_limit() {
        let config = WorkerServiceConfig::default();
        config.validate_http_limits().unwrap();

        assert_eq!(config.body_limits.api, 32 * 1024 * 1024);
        assert_eq!(config.body_limits.mcp, 32 * 1024 * 1024);
        assert_eq!(config.http_server.http2_max_header_list_size, 64 * 1024);
        assert_eq!(
            config.http_server.http2_initial_stream_window_size,
            1024 * 1024
        );
        assert_eq!(
            config.http_server.http2_initial_connection_window_size,
            1024 * 1024
        );
        assert!(!config.http_server.http2_adaptive_window);
        for limits in [
            &config.websocket_limits.invocation_session,
            &config.websocket_limits.connect,
        ] {
            assert_eq!(limits.max_message_size, 32 * 1024 * 1024 + 64 * 1024);
            assert_eq!(limits.max_frame_size, 32 * 1024 * 1024 + 64 * 1024);
            assert_eq!(limits.max_write_buffer_size, 64 * 1024 * 1024);
            let websocket = limits.websocket_config();
            assert_eq!(websocket.max_message_size, Some(limits.max_message_size));
            assert_eq!(websocket.max_frame_size, Some(limits.max_frame_size));
            assert_eq!(
                websocket.max_write_buffer_size,
                limits.max_write_buffer_size
            );
        }
    }

    #[test]
    fn http_limits_are_configurable() {
        let config: WorkerServiceConfig = make_worker_service_config_loader()
            .figment()
            .merge(Toml::string(
                r#"
                [http_server]
                http2_max_header_list_size = 1000
                http2_adaptive_window = true

                [body_limits]
                api = 2000
                mcp = 3000

                [websocket_limits.connect]
                max_message_size = 4000
            "#,
            ))
            .extract()
            .unwrap();

        assert_eq!(config.http_server.http2_max_header_list_size, 1000);
        assert!(config.http_server.http2_adaptive_window);
        assert_eq!(
            config.http_server.http2_initial_stream_window_size,
            1024 * 1024
        );
        assert_eq!(config.body_limits.api, 2000);
        assert_eq!(config.body_limits.mcp, 3000);
        assert_eq!(config.websocket_limits.connect.max_message_size, 4000);
        assert_eq!(
            config.websocket_limits.connect.max_frame_size,
            32 * 1024 * 1024 + 64 * 1024
        );
    }

    #[test]
    fn invocation_session_transport_limits_must_be_above_the_session_message_limit() {
        let mut config = WorkerServiceConfig::default();
        config.websocket_limits.invocation_session.max_message_size = 32 * 1024 * 1024;
        assert!(config.validate_http_limits().is_err());

        let mut config = WorkerServiceConfig::default();
        config.websocket_limits.invocation_session.max_frame_size = 32 * 1024 * 1024;
        assert!(config.validate_http_limits().is_err());

        let mut config = WorkerServiceConfig::default();
        config.websocket_limits.connect.max_message_size = 1024;
        assert!(config.validate_http_limits().is_ok());
    }

    #[test]
    fn websocket_write_buffer_limit_must_exceed_the_write_buffer() {
        let mut config = WorkerServiceConfig::default();
        config.websocket_limits.connect.max_write_buffer_size = 128 * 1024;
        assert!(config.validate_http_limits().is_err());
    }
}

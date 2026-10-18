#!/bin/sh
# Writes the Envoy configuration of the golem-router image from the GOLEM_ROUTER_ variables and
# runs Envoy with it. The arguments of the container are added to the arguments of Envoy, for
# example `--log-level debug`. A `--concurrency` argument replaces GOLEM_ROUTER_CONCURRENCY,
# because Envoy refuses a repeated flag.
#
# Requests for the management host reach the registry service and the management API of the
# worker service. Requests for every other host reach the custom request port of the worker
# service, which serves HTTP API and MCP deployments by domain.
set -eu

fail() {
  echo "golem-router: $*" >&2
  exit 1
}

# The values go into the YAML configuration, so each of them must have the form of its setting.
require() {
  name=$1
  pattern=$2
  eval "value=\${${name}:-}"
  [ -n "${value}" ] || fail "${name} must be set"
  printf '%s' "${value}" | grep -Eqx -- "${pattern}" || fail "${name} has the value '${value}', which is not ${3}"
}

host='[A-Za-z0-9._:-]+'
port='[0-9]{1,5}'
duration='[0-9]+s'

: "${GOLEM_ROUTER_MANAGEMENT_HOST:=}"
: "${GOLEM_ROUTER_REGISTRY_SERVICE_HOST:=}"
: "${GOLEM_ROUTER_WORKER_SERVICE_HOST:=}"
: "${GOLEM_ROUTER_PORT:=8080}"
: "${GOLEM_ROUTER_REGISTRY_SERVICE_PORT:=8081}"
: "${GOLEM_ROUTER_WORKER_SERVICE_PORT:=9005}"
: "${GOLEM_ROUTER_WORKER_SERVICE_CUSTOM_REQUEST_PORT:=9006}"
: "${GOLEM_ROUTER_STREAM_IDLE_TIMEOUT:=3600s}"
: "${GOLEM_ROUTER_DNS_REFRESH_RATE:=10s}"
: "${GOLEM_ROUTER_TRUSTED_HOPS:=0}"
: "${GOLEM_ROUTER_MAX_CONNECTIONS:=50000}"
# Envoy starts one worker thread for each CPU of the host, not of the container limit, and each
# worker opens its own connections to the services.
: "${GOLEM_ROUTER_CONCURRENCY:=2}"
# Empty: no metrics port. The admin interface of Envoy is never reachable from outside the
# container; the metrics port serves only its Prometheus metrics.
: "${GOLEM_ROUTER_STATS_PORT:=}"
admin_port=19000

require GOLEM_ROUTER_MANAGEMENT_HOST '[A-Za-z0-9.-]+' "a host name"
require GOLEM_ROUTER_REGISTRY_SERVICE_HOST "${host}" "a host name or an IP address"
require GOLEM_ROUTER_WORKER_SERVICE_HOST "${host}" "a host name or an IP address"
require GOLEM_ROUTER_PORT "${port}" "a port"
require GOLEM_ROUTER_REGISTRY_SERVICE_PORT "${port}" "a port"
require GOLEM_ROUTER_WORKER_SERVICE_PORT "${port}" "a port"
require GOLEM_ROUTER_WORKER_SERVICE_CUSTOM_REQUEST_PORT "${port}" "a port"
require GOLEM_ROUTER_STREAM_IDLE_TIMEOUT "${duration}" "a number of seconds such as 3600s"
require GOLEM_ROUTER_DNS_REFRESH_RATE "${duration}" "a number of seconds such as 10s"
require GOLEM_ROUTER_TRUSTED_HOPS '[0-9]+' "a number"
require GOLEM_ROUTER_MAX_CONNECTIONS '[0-9]+' "a number"
require GOLEM_ROUTER_CONCURRENCY '[1-9][0-9]*' "a positive number"
if [ -n "${GOLEM_ROUTER_STATS_PORT}" ]; then
  require GOLEM_ROUTER_STATS_PORT "${port}" "a port"
  case "${GOLEM_ROUTER_STATS_PORT}" in
  "${GOLEM_ROUTER_PORT}" | "${admin_port}") fail "GOLEM_ROUTER_STATS_PORT must not be GOLEM_ROUTER_PORT or ${admin_port}, the port of the admin interface" ;;
  esac
fi

# A host name with a trailing dot names the same host, so the management virtual host takes both
# spellings, with and without a port. The router still sends the original authority upstream.
management=${GOLEM_ROUTER_MANAGEMENT_HOST%.}
[ -n "${management}" ] || fail "GOLEM_ROUTER_MANAGEMENT_HOST has the value '.', which is not a host name"
management_domains="\"${management}\", \"${management}:*\", \"${management}.\", \"${management}.:*\""

# Listen on IPv6 and IPv4 where the kernel supports IPv6.
if [ -f /proc/net/if_inet6 ]; then
  listen='address: "::", ipv4_compat: true'
else
  listen='address: 0.0.0.0'
fi

# One cluster of a service. Envoy resolves the host name again every GOLEM_ROUTER_DNS_REFRESH_RATE,
# so the router follows a service that restarts with a new address, and it starts before the
# services can be resolved. The router sends HTTP/2 without TLS to the services, so a request
# that the client cancels ends with a stream reset, which cancels it in the service. The
# WebSockets of the management API need HTTP/1.1, so the worker service API port also has a
# cluster for them. The limits are high, because every client connection of a proxy in front of
# the router can share one upstream connection. The services advertise HTTP/2 windows of 1 MiB,
# so larger windows toward them add no throughput; see the listener for the window sizes.
cluster() {
  name=$1
  address=$2
  port=$3
  protocol=$4
  if [ "${protocol}" = http2 ]; then
    http_config='http2_protocol_options: {initial_stream_window_size: 16777216, initial_connection_window_size: 67108864}'
  else
    http_config='http_protocol_options: {}'
  fi
  cat <<EOF
  - name: ${name}
    cluster_type:
      name: envoy.clusters.dns
      typed_config:
        "@type": type.googleapis.com/envoy.extensions.clusters.dns.v3.DnsCluster
        dns_refresh_rate: ${GOLEM_ROUTER_DNS_REFRESH_RATE}
        dns_lookup_family: V4_PREFERRED
    connect_timeout: 5s
    # Where the host name resolves to several replicas, a call goes to the replica with the
    # fewest open calls, because calls can stay open for a long time.
    lb_policy: LEAST_REQUEST
    load_assignment:
      cluster_name: ${name}
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address: {address: "${address}", port_value: ${port}}
    circuit_breakers:
      thresholds:
      - {max_connections: 10000, max_pending_requests: 10000, max_requests: 10000}
    typed_extension_protocol_options:
      envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
        "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
        explicit_http_config:
          ${http_config}
EOF
}

# A route without a timeout, because invocations, server-sent events, MCP sessions and websockets
# stay open for as long as they need; GOLEM_ROUTER_STREAM_IDLE_TIMEOUT ends a silent one.
route() {
  match=$1
  cluster=$2
  cat <<EOF
              - match: ${match}
                route: {cluster: ${cluster}, timeout: 0s}
EOF
}

# With GOLEM_ROUTER_STATS_PORT, the admin interface listens on the loopback address only, and a
# second listener sends /stats/prometheus to it. Every other path of that listener is 404.
admin() {
  [ -n "${GOLEM_ROUTER_STATS_PORT}" ] || return 0
  cat <<EOF
admin:
  address:
    socket_address: {address: 127.0.0.1, port_value: ${admin_port}}
EOF
}

stats_listener() {
  [ -n "${GOLEM_ROUTER_STATS_PORT}" ] || return 0
  cat <<EOF
  - name: golem_router_stats
    address:
      socket_address: {${listen}, port_value: ${GOLEM_ROUTER_STATS_PORT}}
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          stat_prefix: golem_router_stats
          normalize_path: true
          merge_slashes: true
          route_config:
            virtual_hosts:
            - name: stats
              domains: ["*"]
              routes:
              - match: {path: /stats/prometheus}
                route: {cluster: admin}
              - match: {prefix: /}
                direct_response: {status: 404}
          http_filters:
          - name: envoy.filters.http.router
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
EOF
}

stats_cluster() {
  [ -n "${GOLEM_ROUTER_STATS_PORT}" ] || return 0
  cat <<EOF
  - name: admin
    type: STATIC
    connect_timeout: 1s
    load_assignment:
      cluster_name: admin
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address: {address: 127.0.0.1, port_value: ${admin_port}}
EOF
}

upgrade='headers: [{name: upgrade, present_match: true}]'
workers='safe_regex: {regex: "^/v1/components/[^/]+/workers(/.*)?$"}'
agents='path_separated_prefix: /v1/agents'

config=$(cat <<EOF
$(admin)
overload_manager:
  resource_monitors:
  - name: envoy.resource_monitors.global_downstream_max_connections
    typed_config:
      "@type": type.googleapis.com/envoy.extensions.resource_monitors.downstream_connections.v3.DownstreamConnectionsConfig
      max_active_downstream_connections: ${GOLEM_ROUTER_MAX_CONNECTIONS}
static_resources:
  listeners:
  - name: golem_router
    address:
      socket_address: {${listen}, port_value: ${GOLEM_ROUTER_PORT}}
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          stat_prefix: golem_router
          codec_type: AUTO
          # The router appends the client address to X-Forwarded-For, and it takes the client
          # address and X-Forwarded-Proto from the last GOLEM_ROUTER_TRUSTED_HOPS proxies in front
          # of it. It removes the x-envoy- headers of the client.
          use_remote_address: true
          xff_num_trusted_hops: ${GOLEM_ROUTER_TRUSTED_HOPS}
          # The path is normalized before it is matched, so /%6detrics matches /metrics. An
          # encoded slash stays encoded: agent ids and the paths of HTTP API deployments can hold
          # one.
          normalize_path: true
          merge_slashes: true
          path_with_escaped_slashes_action: KEEP_UNCHANGED
          server_header_transformation: PASS_THROUGH
          stream_idle_timeout: ${GOLEM_ROUTER_STREAM_IDLE_TIMEOUT}
          request_timeout: 0s
          common_http_protocol_options:
            idle_timeout: ${GOLEM_ROUTER_STREAM_IDLE_TIMEOUT}
            # HTTP API deployments and webhooks receive the headers of any client.
            headers_with_underscores_action: ALLOW
          # One HTTP/2 client connection, such as the one of the CLI, carries many calls. A window
          # is the data that a peer can send before the router passes it on, so it is the largest
          # buffer of one slow client or one stalled upload. 16 MiB for each stream still gives
          # about 300 MB/s to one upload at a round trip of 50 ms.
          http2_protocol_options:
            max_concurrent_streams: 10000
            initial_stream_window_size: 16777216
            initial_connection_window_size: 67108864
          upgrade_configs:
          - upgrade_type: websocket
          access_log:
          - name: envoy.access_loggers.stdout
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.access_loggers.stream.v3.StdoutAccessLog
          route_config:
            name: golem_router
            # The worker service selects a deployment by the original authority, with its port.
            # Over HTTP/2 the authority is not a Host header, so the router also sends it as
            # X-Forwarded-Host.
            request_headers_to_add:
            - header: {key: x-forwarded-host, value: "%REQ(:AUTHORITY)%"}
              append_action: OVERWRITE_IF_EXISTS_OR_ADD
            request_headers_to_remove: [x-envoy-external-address]
            virtual_hosts:
            - name: management
              domains: [${management_domains}]
              routes:
              # The metrics of the services stay inside the deployment.
              - match: {path_separated_prefix: /metrics}
                direct_response: {status: 404}
$(route "{${workers}, ${upgrade}}" worker_service_websocket)
$(route "{${agents}, ${upgrade}}" worker_service_websocket)
$(route "{${workers}}" worker_service)
$(route "{${agents}}" worker_service)
$(route "{prefix: /}" registry_service)
            # HTTP API deployments do not serve WebSockets, so the router answers an upgrade
            # request for them with 403. Envoy decides on an upgrade before it routes the request,
            # so it cannot send the request on without the upgrade.
            - name: deployments
              domains: ["*"]
              routes:
              - match: {prefix: /}
                route: {cluster: custom_request, timeout: 0s, upgrade_configs: [{upgrade_type: websocket, enabled: false}]}
          http_filters:
          - name: envoy.filters.http.router
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
              suppress_envoy_headers: true
$(stats_listener)
  clusters:
$(cluster registry_service "${GOLEM_ROUTER_REGISTRY_SERVICE_HOST}" "${GOLEM_ROUTER_REGISTRY_SERVICE_PORT}" http2)
$(cluster worker_service "${GOLEM_ROUTER_WORKER_SERVICE_HOST}" "${GOLEM_ROUTER_WORKER_SERVICE_PORT}" http2)
$(cluster worker_service_websocket "${GOLEM_ROUTER_WORKER_SERVICE_HOST}" "${GOLEM_ROUTER_WORKER_SERVICE_PORT}" http1)
$(cluster custom_request "${GOLEM_ROUTER_WORKER_SERVICE_HOST}" "${GOLEM_ROUTER_WORKER_SERVICE_CUSTOM_REQUEST_PORT}" http2)
$(stats_cluster)
EOF
)

concurrency="--concurrency ${GOLEM_ROUTER_CONCURRENCY}"
for argument in "$@"; do
  case "${argument}" in
  --concurrency | --concurrency=*) concurrency= ;;
  esac
done

# shellcheck disable=SC2086
exec envoy --disable-hot-restart ${concurrency} --config-yaml "${config}" "$@"

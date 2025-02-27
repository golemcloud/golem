$crates = @(
    "golem-api-grpc",
    "golem-client",
    "golem-common",
    "golem-service-base",
    "golem-component-compilation-service",
    "golem-component-service-base",
    "golem-component-service",
    "golem-rib",
    "golem-test-framework",
    "golem-shard-manager",
    "golem-worker-executor-base",
    "golem-worker-executor",
    "golem-worker-service-base",
    "golem-worker-service",
    "integration-tests",
    "wasm-ast",
    "wasm-rpc"
)

foreach ($crate in $crates) {
    Write-Host "Building $crate..."
    & cargo build -p $crate
    # Memory clean up to solve out of memory issues
    [System.GC]::Collect()
}

& cargo make build
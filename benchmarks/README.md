## Reduce noise

Use `UIET=true` to suppress `golem-cli` output.

## Test context creation

To test context creation you can run `cargo run --bin main`. This command starts context and waits for EOF in stdin (Ctrl+D) so you can get logs from pods.

## Run benchmark

To run the benchmark make sure you have the env ready. There should be a target k8s cluster ready and configured as default for your local `kubectl`. You can run benchmark with `cargo run --bin call-echo`.

The only significant difference between `k8s` on `minikube` and `aws` is the routing. Unfortunatelly due to this difference you can't change the environment for your benchmark withour changing benchamark configuration. 

### Use Minikube

To run benchmarks on your local minikube you should start minikube first with configured resources. For instance: `minikube start --memory=32g --cpus=max`. After starting minikube you whould also start `minikube tunnel`.

See [minikube docs](https://minikube.sigs.k8s.io/docs/start/) for details.

Make sure you have `K8sRoutingType::Minikube` configured in your `make_context` step of your benchmark.

### Use AWS

Make sure you have `K8sRoutingType::Ingress` configured in your `make_context` step of your benchmark.

## Benchmark configuration

At the moment file based configuration is not implemented yet - you can configure the benchmark in the source code.

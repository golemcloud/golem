// Package main is a tour of the Golem Go SDK. It defines several agents that
// together exercise every implemented feature:
//
//   - agents in both durable and ephemeral modes (DefineAgent / Spec.Mode);
//   - typed methods bound as closures and as method expressions
//     (DefineMethod / Implement / Bind0);
//   - the type vocabulary — records, lists, maps, options, results, a variant,
//     an enum, and secrets — all derived from ordinary Go types;
//   - config and secrets, declared both per-key (DefineConfig / DefineSecret)
//     and from a struct (ConfigOf / LoadConfig);
//   - HTTP mounts and endpoints (Spec.HTTP / golem.HTTP);
//   - custom state snapshotting (Snapshotter / Spec.Snapshot);
//   - cross-agent RPC in every shape — Call, CallAsync + Future, Trigger,
//     Schedule — plus per-call config overrides (WithConfigValue) and phantom
//     (ephemeral) clients (NewPhantom).
//
// The SDK wires the component exports from its own init(); main stays empty.
package main

func main() {}

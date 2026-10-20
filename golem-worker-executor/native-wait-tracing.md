# Native waits and verification boundaries

Native waits have no dedicated tracing wrapper or wait/window/permit receipt
collector. Enabling a logging filter does not expose their poll state or establish
physical release. There is no replacement telemetry path or test hook.

`NativeIoWait` remains functional cancellation machinery. It composes the existing
interruption sources around native I/O; removing observation does not remove that
cancellation. The operational permit-held flag remains part of eviction safety.
Existing release Prometheus metrics and invocation-result, scheduler and
entity-fence logging remain available for their original purposes.

Tests coordinate at real dependency and peer boundaries: withheld oplog appends or
payloads, accepted TCP connections, WebSocket Upgrade requests, controlled response
bodies and guest promises. They check execution APIs, results, durable history,
accounting and effects. Capacity contention and filesystem or socket checks provide
independent cleanup evidence where the fixture can observe those resources.

A peer withholding data does not prove that a particular future returned
`Poll::Pending`. An incomplete Start does not prove resource creation or a specific
reader-lock ordering. Returned capacity does not count every permit acquisition,
and a result does not identify a private monthly window. Do not reconstruct those
claims from timing or missing logs. Exact polling and ownership properties belong
in colocated tests of the existing production primitive, with explicitly controlled
inputs and observable outputs.

Earlier tracing-based validation is historical. Combined verification of the
instrumentation removal and adapted tests is pending; see the
[integration runbook](integration-test-runbook.md).

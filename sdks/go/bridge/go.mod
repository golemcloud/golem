// The runtime generated bridge clients call an external Golem server through.
//
// It speaks the worker service's REST API over net/http, and the invocation
// session protocol for streaming methods over a WebSocket. Its only
// dependencies are the shared schema model and github.com/coder/websocket, so a
// program that calls Golem from outside gets the wire format and the
// canonical-JSON codec without the guest SDK's WebAssembly bindings.
module github.com/golemcloud/golem/sdks/go/bridge

go 1.23

require github.com/golemcloud/golem/sdks/go/core v0.0.0

require github.com/coder/websocket v1.8.15

// core lives beside this module in the repository. The directive is ignored by
// downstream main modules, which carry their own; it is here so the bridge
// builds from a checkout.
replace github.com/golemcloud/golem/sdks/go/core => ../core

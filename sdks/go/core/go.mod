// The schema model and the canonical-JSON codec, shared by the guest SDK and
// the external bridge runtime.
//
// This module depends on nothing but the standard library. That is the point:
// an external program calling Golem should not have to pull in the guest SDK's
// WebAssembly bindings to speak the wire format, and there must be exactly one
// implementation of canonical JSON in Go.
module github.com/golemcloud/golem/sdks/go/core

go 1.23

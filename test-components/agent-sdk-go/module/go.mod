// Each Go component is its own module, with go.mod in the component directory:
// componentize-go discovers the SDK's WIT by scanning the module and its
// dependencies for componentize-go.toml, which only resolves from the module root.
module agent-sdk-go

go 1.25.5

require github.com/golemcloud/golem/sdks/go/golem v0.0.0

require (
	github.com/apparentlymart/go-userdirs v0.0.0-20200915174352-b0c018a67c13 // indirect
	github.com/bytecodealliance/componentize-go v0.4.0 // indirect
	github.com/gofrs/flock v0.13.0 // indirect
	go.bytecodealliance.org/pkg v0.2.3 // indirect
	golang.org/x/sys v0.37.0 // indirect
)

// componentize-go is pinned per-app through this directive rather than installed
// globally, so `go tool componentize-go` always runs the pinned version.
tool github.com/bytecodealliance/componentize-go

// Relative path to the in-repo SDK (like the Rust test components). The CLI's
// build-check may re-point this at GOLEM_GO_PATH when set.
replace github.com/golemcloud/golem/sdks/go/golem => ../../sdks/go/golem

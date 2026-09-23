module github.com/golemcloud/golem/sdks/go/golem

go 1.27.1

tool github.com/bytecodealliance/componentize-go

require (
	github.com/golemcloud/golem/sdks/go/core v0.0.0
	go.bytecodealliance.org/pkg v0.2.3
)

require (
	github.com/apparentlymart/go-userdirs v0.0.0-20200915174352-b0c018a67c13 // indirect
	github.com/bytecodealliance/componentize-go v0.4.3 // indirect
	github.com/gofrs/flock v0.13.0 // indirect
	golang.org/x/sys v0.37.0 // indirect
)

// core lives beside this module in the repository. The directive is ignored by
// downstream main modules, which carry their own; it is here so the SDK builds
// from a checkout.
replace github.com/golemcloud/golem/sdks/go/core => ../core

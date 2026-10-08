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

// The bindings' async runtime comes from Golem's fork, which resumes a task when
// a Go timer is due (it needs Golem's Go toolchain fork). Downstream main
// modules carry the same replace; the Golem CLI adds it to every component.
replace go.bytecodealliance.org/pkg => github.com/golemcloud/go-pkg v0.2.3-golem.1

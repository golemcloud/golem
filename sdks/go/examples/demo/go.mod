module demo

go 1.25.5

require github.com/golemcloud/golem/sdks/go/golem v0.0.0

require (
	github.com/apparentlymart/go-userdirs v0.0.0-20200915174352-b0c018a67c13 // indirect
	github.com/bytecodealliance/componentize-go v0.4.0 // indirect
	github.com/gofrs/flock v0.13.0 // indirect
	go.bytecodealliance.org/pkg v0.2.3 // indirect
	golang.org/x/sys v0.37.0 // indirect
)

replace github.com/golemcloud/golem/sdks/go/golem => ../../golem

tool github.com/bytecodealliance/componentize-go

package main

import (
	"github.com/golemcloud/golem-go/std"

	// Import this for using the common lib:
	// "app/common-go/lib"
	"app/components-go/component-name/binding"
)

func init() {
	binding.SetExportsPackNameExportsComponentNameApi(&Impl{})
}

type Impl struct {
	counter uint64
}

func (i *Impl) Add(value uint64) {
	std.Init(std.Packages{Os: true, NetHttp: true})

	// Example common lib usage
	// fmt.Println(lib.ExampleCommonFunction())

	i.counter += value
}

func (i *Impl) Get() uint64 {
	std.Init(std.Packages{Os: true, NetHttp: true})

	return i.counter
}

func main() {}

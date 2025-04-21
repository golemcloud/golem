package main

import (
	"github.com/golemcloud/golem-go/std"

	// Import this for using the common lib:
	// "app/common-go/lib"
	componentnameapi "app/components-go/component-name/binding/pack/name-exports/component-name-api"
)

func init() {
	componentnameapi.Exports.Add = Add
	componentnameapi.Exports.Get = Get
}

var counter uint64

func Add(value uint64) {
	std.Init(std.Packages{Os: true, NetHttp: true})

	// Example common lib usage
	// fmt.Println(lib.ExampleCommonFunction())

	counter += value
}

func Get() uint64 {
	std.Init(std.Packages{Os: true, NetHttp: true})

	return counter
}

func main() {}

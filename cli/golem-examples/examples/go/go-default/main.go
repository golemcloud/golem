package main

import (
	"github.com/golemcloud/golem-go/std"

	"pack/name/binding"
)

type RequestBody struct {
	CurrentTotal uint64
}

type ResponseBody struct {
	Message string
}

func init() {
	binding.SetExportsPackNameExportsApi(&ComponentNameImpl{})
}

// total State can be stored in global variables
var total uint64

type ComponentNameImpl struct {
}

// Implementation of the exported interface

func (e *ComponentNameImpl) Add(value uint64) {
	std.Init(std.Packages{Os: true, NetHttp: true})

	total += value
}

func (e *ComponentNameImpl) Get() uint64 {
	std.Init(std.Packages{Os: true, NetHttp: true})

	return total
}

func main() {
}

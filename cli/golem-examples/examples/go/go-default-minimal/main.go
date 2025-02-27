package main

import (
	"pack/name/component_name"
)

func init() {
	component_name.SetExportsPackNameApi(&ComponentNameImpl{})
}

// total State can be stored in global variables
var total uint64

type ComponentNameImpl struct {
}

// Implementation of the exported interface

func (e *ComponentNameImpl) Add(value uint64) {
	total += value
}

func (e *ComponentNameImpl) Get() uint64 {
    return total
}

func main() {
}

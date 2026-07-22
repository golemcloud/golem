// Package main is a Phase-1 smoke component: it exists only to prove the full
// toolchain chain (bindings -> componentize-go build -> valid component ->
// wasmtime instantiate). The guest exports are the generated stubs; real
// dispatch arrives in Phase 2.
package main

import (
	// Linking the generated export glue is what makes the component export the
	// golem:agent/guest world.
	_ "github.com/golemcloud/golem-go"
)

func main() {}

// Package golem is the Golem Go SDK.
//
// Importing this package links the generated golem:agent/guest export glue into
// the component, so an agent's main package only needs:
//
//	import _ "github.com/golemcloud/golem-go"
//
// The generated bindings stay in internal/ (they are an implementation detail
// and must not become public API); Go's internal-package rule means downstream
// modules cannot import them directly, so this package is the linkage point.
package golem

import (
	_ "github.com/golemcloud/golem-go/internal/wit/wit_exports"
)

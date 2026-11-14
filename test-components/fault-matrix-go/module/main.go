// Package main is the barrel for the Go fixture of the streaming fault matrix: it
// blank-imports the fixture so its registration runs on import.
package main

import (
	_ "fault-matrix-go/fixture"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

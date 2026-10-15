// Package main is the barrel for the Go tool provider of the tool streaming
// matrix: it blank-imports the tools so their registration runs on import.
package main

import (
	_ "tool-streaming-go-provider/matrix"
	_ "tool-streaming-go-provider/probe"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

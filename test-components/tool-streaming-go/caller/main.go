// Package main is the barrel for the Go tool caller of the tool streaming
// matrix: it blank-imports the agents so their registration runs on import.
package main

import (
	_ "tool-streaming-go-caller/caller"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

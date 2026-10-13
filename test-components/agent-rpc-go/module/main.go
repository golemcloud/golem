// Package main is the barrel for the Go variant of the cross-language RPC
// fixture: it blank-imports the agents so their registration runs on import.
package main

import (
	_ "agent-rpc-go/streaming"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

// Package main is the barrel for the agent-sdk-go test component: it blank-imports
// each agent package so their init() registers them, and blank-imports the SDK so
// its runtime is linked. This mirrors how the playground and template apps are wired.
package main

import (
	_ "agent-sdk-go/configecho"
	_ "agent-sdk-go/counter"
	_ "agent-sdk-go/httpcall"
	_ "agent-sdk-go/ledger"
	_ "agent-sdk-go/richtypes"
	_ "agent-sdk-go/rpccaller"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

// Package main is the barrel for the agent-sdk-go test component: it blank-imports
// each agent's IMPLEMENTATION package so their registration runs on import, and
// blank-imports the SDK so its runtime is linked. Agents live under agents/<name>/
// (definition = package <name>, implementation = the impl subpackage).
package main

import (
	_ "agent-sdk-go/agents/clock/impl"
	_ "agent-sdk-go/agents/configecho/impl"
	_ "agent-sdk-go/agents/counter/impl"
	_ "agent-sdk-go/agents/httpcall/impl"
	_ "agent-sdk-go/agents/ledger/impl"
	_ "agent-sdk-go/agents/richtypes/impl"
	_ "agent-sdk-go/agents/rpccaller/impl"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

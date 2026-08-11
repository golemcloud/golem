// Package main is the barrel for the agent-sdk-go test component: it blank-imports
// each agent's IMPLEMENTATION package so their registration runs on import, and
// blank-imports the SDK so its runtime is linked. Definitions live in the sibling
// <name>agent packages; implementations in <name>agentimpl.
package main

import (
	_ "agent-sdk-go/configechoagentimpl"
	_ "agent-sdk-go/counteragentimpl"
	_ "agent-sdk-go/httpcallagentimpl"
	_ "agent-sdk-go/ledgeragentimpl"
	_ "agent-sdk-go/richtypesagentimpl"
	_ "agent-sdk-go/rpccalleragentimpl"

	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}

// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Package oplog reads and searches agents' oplogs.
//
// The entries are the host's own types, as the Rust SDK exposes them: switch on
// Entry.Tag() and read a case's parameters with the accessor of the same name.
// Each entry comes with its [Attribution], which tells whether the agent itself or
// an entity inside it executed the entry.
//
//	for e, err := range oplog.Get(id, 0) {
//	    if err != nil { return err }
//	    switch e.Entry.Tag() {
//	    case oplog.AgentInvocationStarted:
//	        p := e.Entry.AgentInvocationStarted()
//	        …
//	    }
//	}
//
// [MustGet] and [MustSearch] are the same iterations without the error, for
// code that would only panic on it.
package oplog

import (
	"iter"

	"github.com/golemcloud/golem/sdks/go/golem"
	oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
)

// Entry is one oplog entry. Its cases are the constants of this package.
type Entry = oplogwit.PublicOplogEntry

// AttributedEntry is an [Entry] with its [Attribution].
type AttributedEntry = oplogwit.AttributedPublicOplogEntry

// Attribution is what executed an entry: the agent itself when Tag() is
// [AttributionAgent], or an entity inside it when Tag() is [AttributionEntity],
// whose invocation Entity() returns.
type Attribution = oplogwit.PublicOplogEntryAttribution

// The cases of an [Attribution], for a switch on Attribution.Tag().
const (
	AttributionAgent  = oplogwit.PublicOplogEntryAttributionAgent
	AttributionEntity = oplogwit.PublicOplogEntryAttributionEntity
)

// The entity invocation that executed an entry. Ancestors are ordered from the
// root entity invocation to the immediate parent of Invocation.
type (
	EntityInvocationContext   = oplogwit.PublicEntityInvocationContext
	EntityInvocation          = oplogwit.PublicEntityInvocation
	AgentEntity               = oplogwit.PublicAgentEntity
	AgentEntityKind           = oplogwit.PublicAgentEntityKind
	EntityCallMode            = oplogwit.PublicEntityCallMode
	EntityInvocationOperation = oplogwit.PublicEntityInvocationOperation
	ToolInvocationOperation   = oplogwit.PublicToolInvocationOperation
)

// OwnerKind is the authoritative execution-owner kind stored at creation.
type OwnerKind = oplogwit.OwnerKind

const (
	OwnerKindComponentAgent        = oplogwit.OwnerKindComponentAgent
	OwnerKindEphemeralExternalTool = oplogwit.OwnerKindEphemeralExternalTool
	OwnerKindOplogProcessor        = oplogwit.OwnerKindOplogProcessor
)

// The parameters of the commonly inspected cases.
type (
	CreateParameters                  = oplogwit.CreateParameters
	AgentInvocationStartedParameters  = oplogwit.AgentInvocationStartedParameters
	AgentInvocationFinishedParameters = oplogwit.AgentInvocationFinishedParameters
	AgentInvocationFailedParameters   = oplogwit.AgentInvocationFailedParameters
	AgentInvocationSettledParameters  = oplogwit.AgentInvocationSettledParameters
	ErrorParameters                   = oplogwit.ErrorParameters
	LogParameters                     = oplogwit.LogParameters
	// Timestamp is the time of an entry: seconds and nanoseconds since the epoch.
	Timestamp = oplogwit.Datetime
)

// The caller of a recorded invocation: the principal that started the chain, the principal
// that made the call, and the tokens presented at the entry point. The oplog never gives a
// user's claims to a guest.
type (
	PublicCaller                = oplogwit.PublicCaller
	PublicPrincipal             = oplogwit.PublicPrincipal
	PublicPrincipalRef          = oplogwit.PublicPrincipalRef
	PublicUserPrincipal         = oplogwit.PublicUserPrincipal
	PublicAccountPrincipal      = oplogwit.PublicAccountPrincipal
	PublicAccountAuthentication = oplogwit.PublicAccountAuthentication
	PublicAgentPrincipal        = oplogwit.PublicAgentPrincipal
	PublicToolPrincipal         = oplogwit.PublicToolPrincipal
	PublicTokenProvenance       = oplogwit.PublicTokenProvenance
)

// SearchHit is an entry [Search] found, with its index in the oplog and its
// [Attribution].
type SearchHit struct {
	Index       uint64
	Attribution Attribution
	Entry       Entry
}

// MustGet is [Get], panicking on a failing page.
func MustGet(id golem.AgentID, start uint64) iter.Seq[AttributedEntry] {
	return func(yield func(AttributedEntry) bool) {
		for e, err := range Get(id, start) {
			if err != nil {
				panic(err)
			}
			if !yield(e) {
				return
			}
		}
	}
}

// MustSearch is [Search], panicking on a failing page.
func MustSearch(id golem.AgentID, text string) iter.Seq[SearchHit] {
	return func(yield func(SearchHit) bool) {
		for h, err := range Search(id, text) {
			if err != nil {
				panic(err)
			}
			if !yield(h) {
				return
			}
		}
	}
}

// Get reads an agent's oplog from the entry at index start. Pages are fetched
// as the loop advances; a failing page is yielded as an error and ends the
// iteration.
func Get(id golem.AgentID, start uint64) iter.Seq2[AttributedEntry, error] {
	return hostGet(id, start)
}

// Search finds the entries of an agent's oplog that match text, in pages like
// [Get].
func Search(id golem.AgentID, text string) iter.Seq2[SearchHit, error] { return hostSearch(id, text) }

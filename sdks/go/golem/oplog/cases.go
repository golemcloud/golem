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

package oplog

import oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"

// The cases of an [Entry], for a switch on Entry.Tag(). Each has an accessor of
// the same name on Entry that returns its parameters.
const (
	// The initial agent oplog entry
	Create = oplogwit.PublicOplogEntryCreate
	// Marks the start of a durable host call (or scope such as a batched-write).
	Start = oplogwit.PublicOplogEntryStart
	// Marks the successful completion of a durable host call (or scope) started by a matching `Start`.
	End = oplogwit.PublicOplogEntryEnd
	// Marks that a durable host call started by a matching `Start` was cancelled
	// (e.g. dropped from a `select!`) before producing a final response.
	Cancelled = oplogwit.PublicOplogEntryCancelled
	// The agent has been invoked
	AgentInvocationStarted = oplogwit.PublicOplogEntryAgentInvocationStarted
	// The agent has completed an invocation
	AgentInvocationFinished = oplogwit.PublicOplogEntryAgentInvocationFinished
	// Agent suspended
	Suspend = oplogwit.PublicOplogEntrySuspend
	// Agent failed
	Error = oplogwit.PublicOplogEntryError
	// A previously failed startup or replay completed successfully.
	RecoverySucceeded = oplogwit.PublicOplogEntryRecoverySucceeded
	// Marker entry added when get-oplog-index is called from the agent, to make the jumping behavior
	// more predictable.
	NoOp = oplogwit.PublicOplogEntryNoOp
	// The agent needs to recover up to the given target oplog index and continue running from
	// the source oplog index from there
	// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
	// ignore all recorded operations in between.
	Jump = oplogwit.PublicOplogEntryJump
	// Indicates that the agent has been interrupted at this point.
	// Only used to recompute the agent's (cached) status, has no effect on execution.
	Interrupted = oplogwit.PublicOplogEntryInterrupted
	// Indicates that the agent has been exited using WASI's exit function.
	Exited = oplogwit.PublicOplogEntryExited
	// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
	// recovery except if there is a corresponding `EndAtomicRegion` entry.
	BeginAtomicRegion = oplogwit.PublicOplogEntryBeginAtomicRegion
	// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
	// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
	// compaction.
	EndAtomicRegion = oplogwit.PublicOplogEntryEndAtomicRegion
	// An invocation request arrived while the agent was busy
	PendingAgentInvocation = oplogwit.PublicOplogEntryPendingAgentInvocation
	// An update request arrived and will be applied as soon the agent restarts
	PendingUpdate = oplogwit.PublicOplogEntryPendingUpdate
	// An update was successfully applied
	SuccessfulUpdate = oplogwit.PublicOplogEntrySuccessfulUpdate
	// An update failed to be applied
	FailedUpdate = oplogwit.PublicOplogEntryFailedUpdate
	// Increased total linear memory size
	GrowMemory = oplogwit.PublicOplogEntryGrowMemory
	// Created a resource instance
	CreateResource = oplogwit.PublicOplogEntryCreateResource
	// Dropped a resource instance
	DropResource = oplogwit.PublicOplogEntryDropResource
	// The agent emitted a log message
	Log = oplogwit.PublicOplogEntryLog
	// The agent's has been restarted, forgetting all its history
	Restart = oplogwit.PublicOplogEntryRestart
	// An unfinished durable invocation was admitted to resume
	Resumed = oplogwit.PublicOplogEntryResumed
	// Activates a plugin
	ActivatePlugin = oplogwit.PublicOplogEntryActivatePlugin
	// Deactivates a plugin
	DeactivatePlugin = oplogwit.PublicOplogEntryDeactivatePlugin
	// Revert an agent to a previous state
	Revert = oplogwit.PublicOplogEntryRevert
	// Cancel a pending invocation
	CancelPendingInvocation = oplogwit.PublicOplogEntryCancelPendingInvocation
	// Begins a transaction operation
	BeginRemoteTransaction = oplogwit.PublicOplogEntryBeginRemoteTransaction
	// Pre-Commit of the transaction, indicating that the transaction will be committed
	PreCommitRemoteTransaction = oplogwit.PublicOplogEntryPreCommitRemoteTransaction
	// Pre-Rollback of the transaction, indicating that the transaction will be rolled back
	PreRollbackRemoteTransaction = oplogwit.PublicOplogEntryPreRollbackRemoteTransaction
	// Committed transaction operation, indicating that the transaction was committed
	CommittedRemoteTransaction = oplogwit.PublicOplogEntryCommittedRemoteTransaction
	// Rolled back transaction operation, indicating that the transaction was rolled back
	RolledBackRemoteTransaction = oplogwit.PublicOplogEntryRolledBackRemoteTransaction
	// A snapshot of the worker's state
	Snapshot = oplogwit.PublicOplogEntrySnapshot
	// Checkpoint for oplog processor plugin delivery tracking
	OplogProcessorCheckpoint = oplogwit.PublicOplogEntryOplogProcessorCheckpoint
	// Sets or overwrites a named retry policy
	SetRetryPolicy = oplogwit.PublicOplogEntrySetRetryPolicy
	// Removes a named retry policy by name
	RemoveRetryPolicy = oplogwit.PublicOplogEntryRemoveRetryPolicy
	// Durable queue entry for pending permission-card work
	CardEventQueued = oplogwit.PublicOplogEntryCardEventQueued
	// Records successful installation of a permission card into the agent wallet
	CardInstalled = oplogwit.PublicOplogEntryCardInstalled
	// Records failed installation of a permission card into the agent wallet
	CardInstallFailed = oplogwit.PublicOplogEntryCardInstallFailed
	// Records that a permission card used by the agent has been revoked
	CardRevoked = oplogwit.PublicOplogEntryCardRevoked
	// Records that a permission card used by the agent has expired
	CardExpired = oplogwit.PublicOplogEntryCardExpired
	// A durably recorded frame of a host-owned stream (e.g. an outgoing HTTP request body),
	// attached to the durable host call identified by its start entry's index
	HostStreamFrame = oplogwit.PublicOplogEntryHostStreamFrame
	// Registers a durable stream before exposing its handle
	StreamRegistered = oplogwit.PublicOplogEntryStreamRegistered
	// Records committed durable stream values or a packed-u8 batch
	StreamItems = oplogwit.PublicOplogEntryStreamItems
	// Records a durable stream end terminal
	StreamEnd = oplogwit.PublicOplogEntryStreamEnd
	// Records a durable stream cancellation terminal
	StreamCancel = oplogwit.PublicOplogEntryStreamCancel
	// Records durable Stream Session state and consumer-journal facts
	StreamSession = oplogwit.PublicOplogEntryStreamSession
	// The successful completion of the durable host call started by the matching `start`
	// was persisted, but its response was never delivered to the agent (the agent dropped
	// the completion future after the `end` was recorded)
	CompletionDiscarded = oplogwit.PublicOplogEntryCompletionDiscarded
	// The successful completion of the durable host call started by the matching `start`
	// was delivered to the agent at this point in the recorded execution
	CompletionDelivered = oplogwit.PublicOplogEntryCompletionDelivered
)

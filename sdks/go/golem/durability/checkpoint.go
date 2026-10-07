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

package durability

// OplogIndex returns the index of the running agent's latest oplog entry.
func OplogIndex() uint64 { return oplogHost.getOplogIndex() }

// SetOplogIndex moves the running agent back to an earlier oplog index: the
// entries after it are discarded and execution continues live from there. It
// does not return.
func SetOplogIndex(index uint64) {
	oplogHost.setOplogIndex(index)
	panic("golem: unreachable: the oplog index was set")
}

// Checkpoint is a point in the running agent's execution to go back to.
// Reverting discards everything recorded since and runs again from there, live —
// the way to retry a side effect whose outcome was not acceptable:
//
//	cp := durability.NewCheckpoint()
//	quote := cp.Must(fetchQuote()) // reverts, and so fetches again, on an error
//	cp.AssertOrRevert(quote.Price < limit)
//
// A revert rewinds the whole agent, including goroutines awaiting something at
// the time.
type Checkpoint struct{ index uint64 }

// NewCheckpoint captures the current point of execution.
func NewCheckpoint() Checkpoint { return Checkpoint{index: OplogIndex()} }

// Revert goes back to the checkpoint. It does not return.
func (c Checkpoint) Revert() { SetOplogIndex(c.index) }

// AssertOrRevert goes back to the checkpoint unless ok.
func (c Checkpoint) AssertOrRevert(ok bool) {
	if !ok {
		c.Revert()
	}
}

// Must returns v, or goes back to the checkpoint if err is not nil. It takes a
// call's results directly: cp.Must(fetchQuote()).
func (c Checkpoint) Must[T any](v T, err error) T {
	if err != nil {
		c.Revert()
	}
	return v
}

// MustRun calls f and returns its value, or goes back to the checkpoint if f
// returns an error.
func (c Checkpoint) MustRun[T any](f func() (T, error)) T { return c.Must(f()) }

// WithCheckpoint captures a checkpoint, runs f with it, and goes back to it if f
// returns an error.
func WithCheckpoint[T any](f func(cp Checkpoint) (T, error)) T {
	cp := NewCheckpoint()
	return cp.Must(f(cp))
}

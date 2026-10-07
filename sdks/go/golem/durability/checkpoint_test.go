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

import (
	"errors"
	"testing"
)

type fakeOplog struct {
	index    uint64
	reverted []uint64
}

func (h *fakeOplog) getOplogIndex() uint64      { return h.index }
func (h *fakeOplog) setOplogIndex(index uint64) { h.reverted = append(h.reverted, index) }

func withFakeOplog(t *testing.T, h *fakeOplog) {
	t.Helper()
	prev := oplogHost
	oplogHost = h
	t.Cleanup(func() { oplogHost = prev })
}

// reverts runs f and reports the index a revert went to, if any.
func reverts(h *fakeOplog, f func()) (index uint64, reverted bool) {
	before := len(h.reverted)
	func() {
		defer func() { _ = recover() }()
		f()
	}()
	if len(h.reverted) > before {
		return h.reverted[len(h.reverted)-1], true
	}
	return 0, false
}

func TestCheckpointRevertsOnlyOnFailure(t *testing.T) {
	h := &fakeOplog{index: 41}
	withFakeOplog(t, h)
	cp := NewCheckpoint()
	h.index = 50

	if _, reverted := reverts(h, func() { _ = cp.Must(1, nil) }); reverted {
		t.Fatal("Must reverted without an error")
	}
	if i, reverted := reverts(h, func() { _ = cp.Must(0, errors.New("no")) }); !reverted || i != 41 {
		t.Fatalf("Must: reverted=%v to %d, want the checkpoint's 41", reverted, i)
	}
	if _, reverted := reverts(h, func() { _ = cp.MustRun(func() (int, error) { return 0, errors.New("no") }) }); !reverted {
		t.Fatal("MustRun did not revert on an error")
	}
	if _, reverted := reverts(h, func() { cp.AssertOrRevert(true) }); reverted {
		t.Fatal("AssertOrRevert reverted on true")
	}
	if _, reverted := reverts(h, func() { cp.AssertOrRevert(false) }); !reverted {
		t.Fatal("AssertOrRevert did not revert on false")
	}
	h.index = 60
	if i, _ := reverts(h, func() {
		WithCheckpoint(func(Checkpoint) (int, error) { return 0, errors.New("no") })
	}); i != 60 {
		t.Fatalf("WithCheckpoint reverted to %d, want the index it captured, 60", i)
	}
	if got := WithCheckpoint(func(Checkpoint) (int, error) { return 3, nil }); got != 3 {
		t.Fatalf("WithCheckpoint = %d", got)
	}
}

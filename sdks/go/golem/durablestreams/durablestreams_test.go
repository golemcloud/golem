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

package durablestreams

import (
	"errors"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	wire "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_durable_streams"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type item struct{ N int }

func batch(payload, next string, upToDate, closed bool) wire.DurableStreamBatch {
	return wire.DurableStreamBatch{
		Payload: []byte(payload), ContentType: "application/json",
		Next:     wire.DurableStreamCheckpoint{Offset: next, Cursor: witTypes.None[string]()},
		UpToDate: upToDate, Closed: closed,
	}
}

func transient(kind wire.DurableStreamErrorKind) *Error {
	return errorFromWire(wire.DurableStreamError{
		Kind: kind, Message: "try later",
		RetryAfterMs: witTypes.None[uint64](), ProducerEpoch: witTypes.None[uint64](), ExpectedSequence: witTypes.None[uint64](),
	})
}

// scripted replays outcomes and records the requests it saw.
func scripted(t *testing.T, outcomes ...any) (readCall, *[]wire.DurableStreamReadRequest) {
	t.Helper()
	var seen []wire.DurableStreamReadRequest
	return func(r wire.DurableStreamReadRequest) (wire.DurableStreamBatch, *Error) {
		seen = append(seen, r)
		if len(outcomes) == 0 {
			t.Fatal("read past the script")
		}
		next := outcomes[0]
		outcomes = outcomes[1:]
		if e, ok := next.(*Error); ok {
			return wire.DurableStreamBatch{}, e
		}
		return next.(wire.DurableStreamBatch), nil
	}, &seen
}

// TestAReaderDeliversBatchesAndPromotesCheckpoints — items arrive in order, a
// batch's checkpoint is promoted only once it is drained, reading starts with
// catch-up and switches transport at the tail, transient failures are retried,
// and a closed stream ends.
func TestAReaderDeliversBatchesAndPromotesCheckpoints(t *testing.T) {
	read, seen := scripted(t,
		batch(`[{"N":1},{"N":2}]`, "o1", false, false),
		transient(wire.DurableStreamErrorKindUnavailable),
		batch(`[]`, "o2", true, false),
		batch(`[{"N":3}]`, "o3", true, true),
	)
	r := newReader[item]("", ReadOptions{IdleDelay: time.Millisecond, Retry: RetryOptions{MaxRetries: 2, InitialDelay: time.Millisecond, MaxDelay: time.Millisecond}}, wire.DurableStreamModeJson, ReadJSON[item]("", ReadOptions{}).decode)
	r.read = read

	first, ok, err := r.Next()
	if err != nil || !ok || first.N != 1 || r.Checkpoint().Offset != "-1" {
		t.Fatalf("first item %v %v %v at %s", first, ok, err, r.Checkpoint().Offset)
	}
	rest, err := r.Collect()
	if err != nil || len(rest) != 2 || rest[0].N != 2 || rest[1].N != 3 {
		t.Fatalf("rest %v, %v", rest, err)
	}
	if r.Checkpoint().Offset != "o3" {
		t.Errorf("checkpoint %s", r.Checkpoint().Offset)
	}
	reqs := *seen
	if reqs[0].Transport != wire.DurableStreamTransportCatchUp || reqs[0].Checkpoint.Offset != "-1" {
		t.Errorf("first request %+v", reqs[0])
	}
	if last := reqs[len(reqs)-1]; last.Transport != wire.DurableStreamTransportLongPoll || last.ContentType.Some() != "application/json" {
		t.Errorf("tail request %+v", last)
	}
}

func TestAReaderStopsOnPermanentErrorsAndBadCheckpoints(t *testing.T) {
	read, _ := scripted(t, transient(wire.DurableStreamErrorKindNotFound))
	r := ReadBytes("", ReadOptions{})
	r.read = read
	if _, _, err := r.Next(); err == nil || err.(*Error).Kind != KindNotFound {
		t.Errorf("a missing stream gave %v", err)
	}

	read, _ = scripted(t, batch("", "now", true, false))
	r = ReadBytes("", ReadOptions{})
	r.read = read
	if _, _, err := r.Next(); err == nil || err.(*Error).Kind != KindProtocolError {
		t.Errorf("an unresolved checkpoint gave %v", err)
	}
}

// TestAWriterSequencesAndRetriesItsAppends — appends carry consecutive
// sequence numbers, a transient failure is retried, an append cannot overtake
// a pending one, and a bad acknowledgement is refused.
func TestAWriterSequencesAndRetriesItsAppends(t *testing.T) {
	var seen []wire.DurableStreamAppendRequest
	fail := true
	w := &Writer{producer: Producer{ID: "p"}, retry: RetryOptions{MaxRetries: 2, InitialDelay: time.Millisecond, MaxDelay: time.Millisecond}}
	w.append = func(r wire.DurableStreamAppendRequest) (wire.DurableStreamAppendReceipt, *Error) {
		seen = append(seen, r)
		if fail {
			fail = false
			return wire.DurableStreamAppendReceipt{}, transient(wire.DurableStreamErrorKindTimeout)
		}
		return wire.DurableStreamAppendReceipt{NextOffset: witTypes.Some("x"), Sequence: r.Sequence, Closed: r.Close}, nil
	}

	if receipt, err := AppendJSON(w, []item{{1}, {2}}, false); err != nil || receipt.Sequence != 0 {
		t.Fatalf("first append %+v, %v", receipt, err)
	}
	if got := seen[0].Payload.Json(); len(got) != 2 || got[0] != `{"N":1}` || len(seen) != 2 {
		t.Errorf("payload %v after %d attempts", got, len(seen))
	}
	if receipt, err := w.Close(); err != nil || !receipt.Closed || w.Producer().Sequence != 2 {
		t.Fatalf("close %+v, %v", receipt, err)
	}
	if _, err := w.AppendBytes([]byte("late"), false); err == nil || err.(*Error).Kind != KindClosed {
		t.Errorf("appending after close gave %v", err)
	}

	w = &Writer{producer: Producer{ID: "p"}, retry: DefaultRetry}
	w.append = func(r wire.DurableStreamAppendRequest) (wire.DurableStreamAppendReceipt, *Error) {
		return wire.DurableStreamAppendReceipt{Sequence: r.Sequence + 1}, nil
	}
	if _, err := w.AppendBytes([]byte("x"), false); err == nil || err.(*Error).Kind != KindProducerDiverged || !w.HasPending() {
		t.Errorf("a diverged acknowledgement gave %v", err)
	}
	if _, err := w.AppendBytes([]byte("y"), false); err == nil || err.(*Error).Kind != KindSequenceConflict {
		t.Errorf("overtaking a pending append gave %v", err)
	}
	if _, err := w.AppendBytes(nil, true); !errors.As(err, new(*Error)) {
		t.Errorf("an error that is not an *Error: %v", err)
	}
}

func TestRetryDelaysBackOffAndHonourRetryAfter(t *testing.T) {
	r := RetryOptions{MaxRetries: 3, InitialDelay: 100 * time.Millisecond, MaxDelay: 300 * time.Millisecond}
	e := transient(wire.DurableStreamErrorKindRateLimited)
	for failures, want := range []time.Duration{100, 200, 300} {
		if got, ok := r.delay(e, uint32(failures)); !ok || got != want*time.Millisecond {
			t.Errorf("failure %d: %v %v", failures, got, ok)
		}
	}
	if _, ok := r.delay(e, 3); ok {
		t.Error("retried past the budget")
	}
	e.RetryAfter = golem.Some(time.Second)
	if got, _ := r.delay(e, 0); got != time.Second {
		t.Errorf("retry-after ignored: %v", got)
	}
}

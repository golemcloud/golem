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

package bridge

import (
	"encoding/json"
	"errors"
	"reflect"
	"strconv"
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

func TestStreamEndDirectionAndOutcomeMatrix(t *testing.T) {
	for _, direction := range []string{"input", "output"} {
		for _, outcome := range []map[string]any{
			{"kind": "finished"},
			{"kind": "failed", "error": map[string]any{"kind": "interrupted", "message": "disk gone"}},
			{"kind": "consumer-cancelled", "details": nil},
			{"kind": "consumer-cancelled", "details": "reader gone"},
		} {
			data, err := encodeMessage("stream-end", map[string]any{
				"channel": 1, "direction": direction, "sequence": "0", "outcome": outcome,
			})
			if err != nil {
				t.Fatal(err)
			}
			_, m, err := parseMessage(data)
			if err != nil {
				t.Fatal(err)
			}
			consumer := outcome["kind"] == "consumer-cancelled"
			for _, server := range []bool{false, true} {
				_, _, err := m.streamEnd(server)
				want := consumer == (direction == "output")
				if server {
					want = true
				}
				if (err == nil) != want {
					t.Errorf("server=%v direction=%s outcome=%v: %v", server, direction, outcome, err)
				}
			}
		}
	}
}

func TestMalformedStreamTerminalsAreRejected(t *testing.T) {
	for _, outcome := range []string{
		`{"kind":"unknown"}`,
		`{"kind":"finished","extra":true}`,
		`{"kind":"failed"}`,
		`{"kind":"failed","error":{"kind":"unknown"}}`,
		`{"kind":"failed","error":{"kind":"cancelled","message":"extra"}}`,
		`{"kind":"failed","error":{"kind":"interrupted","message":null}}`,
		`{"kind":"failed","error":{"kind":"trapped"}}`,
		`{"kind":"failed","error":{"kind":"failed","value":{"graph":{"root":{"kind":"u8","value":{}}},"value":{"kind":"string","value":"wrong type"}}}}`,
		`{"kind":"failed","error":{"kind":"failed","value":{"graph":{"root":{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}},"value":{"kind":"stream","value":{"streamToken":"not-allowed"}}}}}`,
		`{"kind":"consumer-cancelled","details":42}`,
		`{"kind":"consumer-cancelled","reason":"old"}`,
	} {
		t.Run(outcome, func(t *testing.T) {
			data, err := encodeMessage("stream-end", map[string]any{
				"channel": 2, "direction": "output", "sequence": "0", "outcome": json.RawMessage(outcome),
			})
			if err != nil {
				t.Fatal(err)
			}
			_, m, err := parseMessage(data)
			if err != nil {
				t.Fatal(err)
			}
			if _, _, err := m.streamEnd(true); err == nil {
				t.Fatal("accepted malformed terminal")
			}
		})
	}
}

func TestStreamEndMustMatchMappedChannelDirection(t *testing.T) {
	for _, direction := range []string{"input", "output"} {
		data, err := encodeMessage("stream-end", map[string]any{
			"channel": 1, "direction": direction, "sequence": "0",
			"outcome": map[string]any{"kind": "consumer-cancelled", "details": nil},
		})
		if err != nil {
			t.Fatal(err)
		}
		_, m, err := parseMessage(data)
		if err != nil {
			t.Fatal(err)
		}
		mapped := "input"
		if direction == "input" {
			mapped = "output"
		}
		c := &sessionConn{}
		s := &session{conn: c, channels: map[uint32]string{1: "stream"}, directions: map[string]string{"stream": mapped}}
		if err := s.receiveEnd(c, m); err == nil {
			t.Fatalf("accepted %s terminal on %s channel", direction, mapped)
		}
	}
}

func TestServerInputTerminalWinsSourceFailure(t *testing.T) {
	for _, outcome := range []map[string]any{
		{"kind": "consumer-cancelled", "details": "reader gone"},
		{"kind": "failed", "error": map[string]any{"kind": "trapped", "message": "callee trapped"}},
		{"kind": "failed", "error": map[string]any{"kind": "interrupted", "message": "protocol failure"}},
	} {
		c := &sessionConn{}
		s := &session{conn: c, channels: map[uint32]string{1: "in"}, directions: map[string]string{"in": "input"}, unacked: 5}
		var stopped string
		in := &inputStream{
			s: s, next: 1, pending: &pendingInput{count: 1, bytes: 5}, wake: make(chan struct{}, 1),
			h: &inputHandle{stop: func(reason string) { stopped = reason }},
		}
		s.byToken = map[string]*inputStream{"in": in}
		data, err := encodeMessage("stream-end", map[string]any{
			"channel": 1, "direction": "input", "sequence": "0",
			"outcome": outcome,
		})
		if err != nil {
			t.Fatal(err)
		}
		_, m, err := parseMessage(data)
		if err != nil {
			t.Fatal(err)
		}
		if err := s.receiveEnd(c, m); err != nil {
			t.Fatal(err)
		}
		if !in.cancelled || !in.ended || in.pending != nil || s.unacked != 0 || stopped == "" {
			t.Fatalf("cancellation did not release input: %+v, stopped=%q", in, stopped)
		}
		if details, ok := outcome["details"].(string); ok && stopped != details {
			t.Fatalf("consumer cancellation lost its details: %q", stopped)
		}
		// The source may finish concurrently with the cancellation. It must not
		// replace the consumer's terminal with a producer failure.
		if in.reserve(&pendingInput{terminal: true, failure: errors.New("late failure")}) || len(c.queue) != 0 {
			t.Fatal("source failure won after consumer cancellation")
		}
	}
}

func TestOutputConsumerCancellationDetailsRemainInspectable(t *testing.T) {
	text := "reader gone"
	for _, details := range []*string{nil, &text} {
		server := newFakeServer(t, func(c *fakeConn) {
			start := c.expect("invocationStart")
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputEnd(c, 2, 0, "", map[string]any{"kind": "consumer-cancelled", "details": details})
			finished(c)
			c.waitClose()
		})
		ctx := ctxT(t)
		out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
		if err != nil {
			t.Fatal(err)
		}
		_, err = out.Collect(ctx)
		var cancelled *StreamCancelled
		if !errors.As(err, &cancelled) || !reflect.DeepEqual(cancelled.Details, details) {
			t.Fatalf("lost consumer cancellation details: %v", err)
		}
	}
}

func TestOutputFailureVariantsRemainInspectable(t *testing.T) {
	// These are the six producer failure cases in the public Rust contract.
	for _, kind := range []StreamErrorKind{
		StreamErrorFailed, StreamErrorTrapped, StreamErrorCancelled,
		StreamErrorAbandoned, StreamErrorInterrupted, StreamErrorResourceExhausted,
	} {
		t.Run(string(kind), func(t *testing.T) {
			failure := map[string]any{"kind": kind}
			switch kind {
			case StreamErrorFailed:
				failure["value"] = map[string]any{
					"graph": json.RawMessage(`{"root":{"kind":"string","value":{}}}`),
					"value": map[string]any{"kind": "string", "value": "declared failure"},
				}
			case StreamErrorTrapped, StreamErrorInterrupted, StreamErrorResourceExhausted:
				failure["message"] = "diagnostic"
			}
			server := newFakeServer(t, func(c *fakeConn) {
				start := c.expect("invocationStart")
				accept(c, start, "sess")
				streamResult(c, 2, "out", u32StreamGraph)
				outputEnd(c, 2, 0, "end", map[string]any{"kind": "failed", "error": failure})
				finished(c)
				c.waitClose()
			})
			ctx := ctxT(t)
			out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
			if err != nil {
				t.Fatal(err)
			}
			_, err = out.Collect(ctx)
			var got *StreamError
			if !errors.As(err, &got) || got.Kind != kind {
				t.Fatalf("got %v", err)
			}
			if kind == StreamErrorFailed {
				if !reflect.DeepEqual(got.Value, schema.StringValue{Value: "declared failure"}) {
					t.Fatalf("lost typed failure: %+v", got)
				}
				if err := schema.NewRef(got.Graph).Validate(got.Value); err != nil {
					t.Fatal(err)
				}
			} else if text, ok := failure["message"]; ok && got.Message != text {
				t.Fatalf("lost diagnostic: %+v", got)
			}
		})
	}
}

func TestInputTerminalReplaysUntilDurableAcknowledgement(t *testing.T) {
	for _, packed := range []bool{false, true} {
		for _, failed := range []bool{false, true} {
			t.Run("packed="+strconv.FormatBool(packed)+"/failed="+strconv.FormatBool(failed), func(t *testing.T) {
				var params schema.SchemaValue
				var sourceErr error
				if failed {
					sourceErr = errors.New("disk gone")
				}
				if packed {
					w, source := NewAgentStream[uint8]()
					if err := w.Write(7); err != nil {
						t.Fatal(err)
					}
					if err := w.CloseWithError(sourceErr); err != nil {
						t.Fatal(err)
					}
					params = streamParams(EncodeStream(EncodeU8)(source))
				} else {
					w, source := NewAgentStream[uint32]()
					if err := w.Write(7); err != nil {
						t.Fatal(err)
					}
					if err := w.CloseWithError(sourceErr); err != nil {
						t.Fatal(err)
					}
					params = streamParams(EncodeStream(EncodeU32)(source))
				}
				var key any
				var original map[string]any
				prepared := make(chan struct{})
				replayed := make(chan struct{})
				server := newFakeServer(t,
					func(c *fakeConn) {
						start := c.expect("invocationStart")
						key = start.m["idempotencyKey"]
						accept(c, start, "sess", inputMapping(1, provisionalRefs(t, start)[0], "in", 0, false))
						if packed {
							item := c.expect("input-u8")
							if !reflect.DeepEqual(item.binary.payload, []byte{7}) {
								t.Errorf("lost packed items: %v", item.binary)
							}
						} else {
							c.expect("inputStreamItem")
						}
						ack(c, 1, 1, false)
						original = c.expect("stream-end").m
						want := map[string]any{"kind": "finished"}
						if failed {
							want = map[string]any{"kind": "failed", "error": map[string]any{"kind": "interrupted", "message": "disk gone"}}
						}
						if original["sequence"] != "1" || original["direction"] != "input" || !reflect.DeepEqual(original["outcome"], want) {
							t.Errorf("terminal: %v", original)
						}
						close(prepared)
						c.drop()
					},
					func(c *fakeConn) {
						<-prepared
						resume := c.expect("resumeAttach")
						acceptAs(c, resume, key, "sess2", inputMapping(5, "", "in", 1, false))
						replay := c.expect("stream-end").m
						original["channel"] = float64(5)
						if !reflect.DeepEqual(replay, original) {
							t.Errorf("replay %v, want %v", replay, original)
						}
						ack(c, 5, 1, true)
						// The next resume reports the durable terminal; it must not replay.
						close(replayed)
						c.drop()
					},
					func(c *fakeConn) {
						<-replayed
						resume := c.expect("resumeAttach")
						acceptAs(c, resume, key, "sess3", inputMapping(9, "", "in", 1, true))
						noResult(c)
						finished(c)
						if extra := c.readUntilClose(); extra.kind != "" {
							t.Errorf("replayed acknowledged terminal: %v", extra)
						}
					},
				)
				if err := InvokeStreaming(ctxT(t), server.agent(t), "consume", params); err != nil {
					t.Fatal(err)
				}
			})
		}
	}
}

func TestOversizedSourceFailureFailsTheCallWithoutSendingATerminal(t *testing.T) {
	// Escaping makes this diagnostic exceed the budget even though its UTF-8
	// representation is much smaller than the budget.
	diagnostic := strings.Repeat("\x00", maxUnacked/6)
	for _, packed := range []bool{false, true} {
		t.Run("packed="+strconv.FormatBool(packed), func(t *testing.T) {
			closed := make(chan struct{})
			server := newFakeServer(t, func(c *fakeConn) {
				start := c.expect("invocationStart")
				accept(c, start, "sess", inputMapping(1, provisionalRefs(t, start)[0], "in", 0, false))
				if r := c.readUntilClose(); r.kind != "" {
					t.Errorf("sent an unadmitted failure or its preceding batch: %s", r.kind)
				}
				close(closed)
			})
			var params schema.SchemaValue
			if packed {
				w, source := NewAgentStream[uint8]()
				if err := w.Write(7); err != nil {
					t.Fatal(err)
				}
				if err := w.CloseWithError(errors.New(diagnostic)); err != nil {
					t.Fatal(err)
				}
				params = streamParams(EncodeStream(EncodeU8)(source))
			} else {
				w, source := NewAgentStream[uint32]()
				if err := w.CloseWithError(errors.New(diagnostic)); err != nil {
					t.Fatal(err)
				}
				params = streamParams(EncodeStream(EncodeU32)(source))
			}
			err := InvokeStreaming(ctxT(t), server.agent(t), "consume", params)
			var unsendable *StreamItemError
			if !errors.As(err, &unsendable) || !strings.Contains(err.Error(), "budget") {
				t.Fatalf("oversized source diagnostic returned %v", err)
			}
			<-closed
		})
	}
}

func TestInputFailureAdmissionBudgetBoundary(t *testing.T) {
	diagnostic := "source failed: \"diagnostic\"\n" + strings.Repeat("\x00", 100)
	for _, delta := range []int{-1, 0, 1} {
		t.Run(strconv.Itoa(delta), func(t *testing.T) {
			p := &pendingInput{terminal: true, failure: errors.New(diagnostic)}
			frame, err := p.frame(^uint32(0))
			if err != nil {
				t.Fatal(err)
			}
			charge := len(frame.data)
			otherBytes := maxUnacked - charge + delta
			s := &session{unacked: otherBytes, result: make(chan sessionResult, 1), done: make(chan struct{}), endLife: func() {}}
			in := &inputStream{s: s, wake: make(chan struct{}, 1), pumping: true}
			if admitted := in.reserve(p); admitted != (delta <= 0) {
				t.Fatalf("delta %d admitted=%v", delta, admitted)
			}
			if delta > 0 {
				var unsendable *StreamItemError
				if !errors.As(s.err, &unsendable) || in.pending != nil || s.unacked != otherBytes {
					t.Fatalf("rejected failure was retained: pending=%v unacked=%d err=%v", in.pending, s.unacked, s.err)
				}
				return
			}
			if p.bytes != charge || s.unacked != otherBytes+charge {
				t.Fatal("failure was not charged exactly once")
			}
			if err := in.acknowledge(0, false); err != nil {
				t.Fatal(err)
			}
			if err := in.remap(1234567890, 0, false); err != nil {
				t.Fatal(err)
			}
			replay, err := in.pending.frame(1234567890)
			if err != nil {
				t.Fatal(err)
			}
			_, m, err := parseMessage(replay.data)
			if err != nil {
				t.Fatal(err)
			}
			_, end, err := m.streamEnd(false)
			var failed *StreamError
			if err != nil || !errors.As(end, &failed) || failed.Message != diagnostic {
				t.Fatal("replay lost diagnostic")
			}
			if s.unacked != otherBytes+charge {
				t.Fatal("nonterminal ACK or replay released diagnostic")
			}
			if err := in.acknowledge(0, true); err != nil {
				t.Fatal(err)
			}
			if err := in.acknowledge(0, true); err != nil {
				t.Fatal(err)
			}
			if s.unacked != otherBytes || in.pending != nil {
				t.Fatal("terminal ACK did not release exactly the diagnostic charge")
			}
		})
	}
}

func TestDeferredPackedFailureAccountingAndCancellation(t *testing.T) {
	for _, cancelStage := range []int{-1, 0, 1, 2} {
		t.Run(strconv.Itoa(cancelStage), func(t *testing.T) {
			s := &session{}
			in := &inputStream{s: s, wake: make(chan struct{}, 1), pumping: true, endAfter: true}
			p := &pendingInput{count: 3, lane: laneU8, payload: []byte{1, 2, 3}, bytes: 3, failure: errors.New("disk gone")}
			terminal := &pendingInput{first: 3, terminal: true, failure: p.failure}
			frame, err := terminal.frame(^uint32(0))
			if err != nil {
				t.Fatal(err)
			}
			charge := len(frame.data)
			if !in.reserve(p) || s.unacked != charge+3 || in.failureBytes != charge {
				t.Fatal("deferred diagnostic bypassed budget")
			}
			if cancelStage != 0 {
				if err := in.acknowledge(3, false); err != nil {
					t.Fatal(err)
				}
				if s.unacked != charge || in.failureBytes != charge {
					t.Fatal("batch ACK released the deferred diagnostic")
				}
			}
			if cancelStage != 0 && cancelStage != 1 {
				end, err := in.produce()
				if err != nil {
					t.Fatal(err)
				}
				if !in.reserve(end) || end.bytes != charge || in.failureBytes != 0 || s.unacked != charge {
					t.Fatal("terminal transfer changed charge")
				}
			}
			if cancelStage >= 0 {
				in.cancelled = true
				in.release()
				in.release()
			} else {
				if err := in.remap(7, 3, true); err != nil {
					t.Fatal(err)
				}
			}
			if s.unacked != 0 || in.pending != nil || in.failureBytes != 0 || in.sourceFailure != nil {
				t.Fatal("terminal completion retained or double-released bytes")
			}
		})
	}
}

func TestDeferredFailureCanFinishWhenOtherInputsSpendTheBudget(t *testing.T) {
	s := &session{}
	in := &inputStream{s: s, wake: make(chan struct{}, 1), pumping: true, endAfter: true}
	p := &pendingInput{count: 1, lane: laneU8, payload: []byte{7}, bytes: 1, failure: errors.New("disk gone")}
	if !in.reserve(p) {
		t.Fatal("failed to reserve packed failure")
	}
	if err := in.acknowledge(1, false); err != nil {
		t.Fatal(err)
	}
	charge := s.unacked
	// Ordinary input item admission remains a soft limit. Another source may
	// have read an item while this source was still producing its byte batch.
	s.unacked = maxUnacked - 1
	other := &inputStream{s: s, wake: make(chan struct{}, 1)}
	if !other.reserve(&pendingInput{count: 1, bytes: 2}) || s.unacked != maxUnacked+1 {
		t.Fatal("ordinary item admission changed")
	}
	if !in.idle() {
		t.Fatal("already reserved terminal could not proceed")
	}
	end, err := in.produce()
	if err != nil {
		t.Fatal(err)
	}
	if !in.reserve(end) || s.unacked != maxUnacked+1 {
		t.Fatal("terminal required a second reservation")
	}
	if err := in.acknowledge(1, true); err != nil {
		t.Fatal(err)
	}
	if s.unacked != maxUnacked+1-charge {
		t.Fatal("terminal ACK released another source's bytes")
	}
}

func TestInputFailureIndividualBudgetIsInclusive(t *testing.T) {
	p := &pendingInput{terminal: true, failure: errors.New("")}
	frame, err := p.frame(^uint32(0))
	if err != nil {
		t.Fatal(err)
	}
	p.failure = errors.New(strings.Repeat("x", maxUnacked-len(frame.data)))
	s := &session{}
	in := &inputStream{s: s, wake: make(chan struct{}, 1)}
	if !in.reserve(p) || p.bytes != maxUnacked || s.unacked != maxUnacked {
		t.Fatal("a failure exactly at the byte budget was rejected or mischarged")
	}
	if err := in.acknowledge(0, true); err != nil {
		t.Fatal(err)
	}
	if s.unacked != 0 {
		t.Fatal("full-budget failure was not released")
	}
}

func TestInputFailureFrameBudgetIsInclusive(t *testing.T) {
	p := &pendingInput{terminal: true, failure: errors.New("")}
	frame, err := p.frame(^uint32(0))
	if err != nil {
		t.Fatal(err)
	}
	diagnostic := strings.Repeat("x", maxMessageBytes-len(frame.data))
	p.failure = errors.New(diagnostic)
	frame, err = p.frame(^uint32(0))
	if err != nil || len(frame.data) != maxMessageBytes {
		t.Fatalf("frame at the byte limit: length %d, error %v", len(frame.data), err)
	}
	p.failure = errors.New(diagnostic + "x")
	if _, err := p.frame(^uint32(0)); err == nil {
		t.Fatal("terminal frame over the byte limit was accepted")
	}
}

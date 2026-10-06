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
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// A scripted session server: each connection runs the next script.
type fakeServer struct {
	t       *testing.T
	mu      sync.Mutex
	scripts []func(*fakeConn)
	srv     *httptest.Server
}

func newFakeServer(t *testing.T, scripts ...func(*fakeConn)) *fakeServer {
	f := &fakeServer{t: t, scripts: scripts}
	f.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != sessionEndpoint || r.Header.Get("Authorization") != "Bearer token" {
			http.Error(w, "unexpected request", http.StatusBadRequest)
			return
		}
		ws, err := websocket.Accept(w, r, &websocket.AcceptOptions{Subprotocols: []string{sessionSubprotocol}})
		if err != nil {
			return
		}
		f.mu.Lock()
		if len(f.scripts) == 0 {
			f.mu.Unlock()
			_ = ws.Close(websocket.StatusPolicyViolation, "no more scripts")
			return
		}
		script := f.scripts[0]
		f.scripts = f.scripts[1:]
		f.mu.Unlock()
		ws.SetReadLimit(maxMessageBytes)
		c := &fakeConn{t: t, ws: ws}
		script(c)
	}))
	t.Cleanup(f.srv.Close)
	return f
}

func (f *fakeServer) agent(t *testing.T) *Agent {
	a, err := NewAgent("Target", schema.RecordValue{Fields: []schema.SchemaValue{schema.StringValue{Value: "t1"}}},
		WithConfiguration(Configuration{Server: Custom(f.srv.URL, "token"), AppName: "app", EnvName: "env"}))
	if err != nil {
		t.Fatal(err)
	}
	return a
}

type fakeConn struct {
	t  *testing.T
	ws *websocket.Conn
}

type received struct {
	kind   string
	m      map[string]any
	binary *binaryFrame
}

func (c *fakeConn) read() received {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	kind, data, err := c.ws.Read(ctx)
	if err != nil {
		c.t.Errorf("server read: %v", err)
		return received{}
	}
	if kind == websocket.MessageBinary {
		f, err := decodeBinary(data)
		if err != nil {
			c.t.Errorf("client sent a bad frame: %v", err)
		}
		return received{kind: f.kind, binary: &f}
	}
	var m map[string]any
	if err := json.Unmarshal(data, &m); err != nil {
		c.t.Errorf("client sent bad JSON: %v", err)
	}
	return received{kind: m["type"].(string), m: m}
}

func (c *fakeConn) expect(kind string) received {
	r := c.read()
	if r.kind != kind {
		c.t.Errorf("server got %s %v, want %s", r.kind, r.m, kind)
	}
	return r
}

func (c *fakeConn) send(kind string, members map[string]any) {
	data, err := encodeMessage(kind, members)
	if err != nil {
		c.t.Fatal(err)
	}
	if err := c.ws.Write(context.Background(), websocket.MessageText, data); err != nil {
		c.t.Logf("server write: %v", err)
	}
}

func (c *fakeConn) sendBinary(kind string, channel uint32, sequence uint64, cursor string, payload []byte) {
	meta, _ := json.Marshal(map[string]any{
		"channel": channel, "cursorToken": cursor, "itemCount": itoa(uint64(len(payload))),
		"kind": kind, "sequence": itoa(sequence), "version": 1,
	})
	frame := append([]byte{byte(len(meta) >> 24), byte(len(meta) >> 16), byte(len(meta) >> 8), byte(len(meta))}, meta...)
	frame = append(frame, payload...)
	if err := c.ws.Write(context.Background(), websocket.MessageBinary, frame); err != nil {
		c.t.Logf("server write: %v", err)
	}
}

func (c *fakeConn) drop() { _ = c.ws.CloseNow() }

func (c *fakeConn) waitClose() {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	for {
		if _, _, err := c.ws.Read(ctx); err != nil {
			return
		}
	}
}

// provisionalRefs collects the provisional references in a start's arguments.
func provisionalRefs(t *testing.T, start received) []string {
	raw, _ := json.Marshal(start.m["methodParameters"])
	v, err := schema.UnmarshalWireValue(raw)
	if err != nil {
		t.Fatal(err)
	}
	var refs []string
	_, _ = mapStreams(v, func(sv schema.StreamValue) (schema.SchemaValue, error) {
		refs = append(refs, sv.Handle.(schema.WireStreamRef).ProvisionalRef)
		return sv, nil
	})
	return refs
}

func accept(c *fakeConn, start received, session string, mappings ...map[string]any) {
	acceptAs(c, start, start.m["idempotencyKey"], session, mappings...)
}

// acceptAs accepts an operation that does not carry the idempotency key
// itself, such as a resume.
func acceptAs(c *fakeConn, op received, key any, session string, mappings ...map[string]any) {
	if mappings == nil {
		mappings = []map[string]any{}
	}
	c.send("invocationAccepted", map[string]any{
		"attemptId": op.m["attemptId"], "idempotencyKey": key,
		"mappings": mappings, "sessionToken": session,
	})
}

func inputMapping(channel uint32, provisional, token string, high uint64, terminal bool) map[string]any {
	m := map[string]any{
		"channel": channel, "direction": "input", "streamToken": token,
		"inputHighWater": map[string]any{"sequence": itoa(high), "terminal": terminal},
	}
	if provisional != "" {
		m["provisionalRef"] = provisional
	}
	return m
}

func outputMapping(channel uint32, token string) map[string]any {
	return map[string]any{"channel": channel, "direction": "output", "streamToken": token}
}

func itoa(n uint64) string {
	out, _ := json.Marshal(n)
	return string(out)
}

func ack(c *fakeConn, channel uint32, high uint64, terminal bool) {
	c.send("inputStreamAck", map[string]any{
		"channel": channel, "highestContiguousSequence": itoa(high), "mappings": []any{}, "terminal": terminal,
	})
}

// consumeInput reads an input stream's items and end, acknowledging each.
func consumeInput(c *fakeConn, channel uint32) []any {
	var items []any
	for {
		r := c.read()
		switch r.kind {
		case "inputStreamItem":
			value := r.m["value"].(map[string]any)["value"]
			items = append(items, value)
			ack(c, channel, uint64(len(items)), false)
		case "input-u8":
			for _, b := range r.binary.payload {
				items = append(items, float64(b))
			}
			ack(c, channel, uint64(len(items)), false)
		case "inputStreamEnd":
			ack(c, channel, uint64(len(items)), true)
			return items
		default:
			c.t.Errorf("unexpected %s while reading input", r.kind)
			return items
		}
	}
}

const u32StreamGraph = `{"root":{"kind":"stream","value":{"inner":{"kind":"u32","value":{}}}}}`
const u8StreamGraph = `{"root":{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}}`

func streamResult(c *fakeConn, channel uint32, token, graph string) {
	c.send("invocationResult", map[string]any{
		"mappings": []any{outputMapping(channel, token)},
		"result": map[string]any{
			"graph": json.RawMessage(graph), "kind": "value",
			"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": token}},
		},
	})
}

func outputItem(c *fakeConn, channel uint32, sequence uint64, cursor string, value uint32) {
	c.send("outputStreamItem", map[string]any{
		"channel": channel, "cursorToken": cursor, "mappings": []any{}, "sequence": itoa(sequence),
		"value": map[string]any{"kind": "u32", "value": value},
	})
}

func outputEnd(c *fakeConn, channel uint32, sequence uint64, cursor string, outcome map[string]any) {
	m := map[string]any{"channel": channel, "outcome": outcome, "sequence": itoa(sequence)}
	if cursor != "" {
		m["cursorToken"] = cursor
	}
	c.send("outputStreamEnd", m)
}

func finished(c *fakeConn) {
	c.send("invocationFinished", map[string]any{"outcome": map[string]any{"kind": "success"}})
}

func streamParams(s schema.SchemaValue) schema.SchemaValue {
	return schema.RecordValue{Fields: []schema.SchemaValue{s}}
}

func ctxT(t *testing.T) context.Context {
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	t.Cleanup(cancel)
	return ctx
}

func TestStreamsFlowBothWays(t *testing.T) {
	var got []any
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		selector := start.m["selector"].(map[string]any)
		if selector["method"] != "transform" || selector["agentType"] != "Target" || selector["application"] != "app" {
			t.Errorf("selector %v", selector)
		}
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		got = consumeInput(c, 1)
		streamResult(c, 2, "out", u32StreamGraph)
		outputItem(c, 2, 0, "c0", 10)
		outputItem(c, 2, 1, "c1", 20)
		outputEnd(c, 2, 2, "c2", map[string]any{"kind": "ok"})
		finished(c)
		c.waitClose()
	})
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "transform",
		streamParams(EncodeStream(EncodeU32)(StreamOf[uint32](1, 2, 3))), DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	values, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{10, 20}) {
		t.Fatalf("collected %v, %v", values, err)
	}
	if !reflect.DeepEqual(got, []any{float64(1), float64(2), float64(3)}) {
		t.Fatalf("server received %v", got)
	}
}

// A dropped connection resumes from the last delivered item: nothing is read
// twice and nothing is lost.
func TestAResumedSessionContinuesAfterTheLastDeliveredItem(t *testing.T) {
	readFirst := make(chan struct{})
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			refs := provisionalRefs(t, start)
			accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
			consumeInput(c, 1)
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			<-readFirst
			outputItem(c, 2, 1, "c1", 20)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			close(resumed)
			if resume.m["sessionToken"] != "sess" || !reflect.DeepEqual(resume.m["outputCursors"], []any{"c0"}) {
				t.Errorf("resumed with %v", resume.m)
			}
			acceptAs(c, resume, key, "sess2", inputMapping(5, "", "in", 1, true), outputMapping(6, "out"))
			outputItem(c, 6, 1, "c1", 20)
			outputEnd(c, 6, 2, "c2", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "transform",
		streamParams(EncodeStream(EncodeU32)(StreamOf[uint32](1))), DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	first, ok, err := out.Next(ctx)
	if err != nil || !ok || first != 10 {
		t.Fatalf("first item %v %v %v", first, ok, err)
	}
	close(readFirst)
	<-resumed
	rest, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(rest, []uint32{20}) {
		t.Fatalf("rest %v, %v", rest, err)
	}
}

// Bytes travel packed; a batch replayed after a reconnect skips what the
// reader already has.
func TestPackedBytesResumeMidBatch(t *testing.T) {
	readTwo := make(chan struct{})
	resumed := make(chan struct{})
	var input []any
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			refs := provisionalRefs(t, start)
			accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
			input = consumeInput(c, 1)
			streamResult(c, 2, "out", u8StreamGraph)
			c.sendBinary("output-u8", 2, 0, "c4", []byte{1, 2, 3, 4})
			<-readTwo
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			close(resumed)
			if cursors := resume.m["outputCursors"].([]any); len(cursors) != 0 {
				t.Errorf("a partly read batch has no cursor yet, got %v", cursors)
			}
			acceptAs(c, resume, key, "sess", inputMapping(5, "", "in", 3, true), outputMapping(6, "out"))
			c.sendBinary("output-u8", 6, 0, "c4", []byte{1, 2, 3, 4})
			outputEnd(c, 6, 4, "c5", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "transformBytes",
		streamParams(EncodeStream(EncodeU8)(StreamOf[uint8](7, 8, 9))), DecodeStream(DecodeU8))
	if err != nil {
		t.Fatal(err)
	}
	var got []uint8
	for range 2 {
		b, _, err := out.Next(ctx)
		if err != nil {
			t.Fatal(err)
		}
		got = append(got, b)
	}
	close(readTwo)
	<-resumed
	rest, err := out.Collect(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if got = append(got, rest...); !reflect.DeepEqual(got, []uint8{1, 2, 3, 4}) {
		t.Fatalf("read %v", got)
	}
	if !reflect.DeepEqual(input, []any{float64(7), float64(8), float64(9)}) {
		t.Fatalf("server received %v", input)
	}
}

func TestARejectedCallReportsItsCode(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		c.send("invocationRejected", map[string]any{
			"attemptId": start.m["attemptId"], "code": "validation-error", "message": "bad", "retryable": false,
		})
		c.waitClose()
	})
	_, err := CallStreaming(ctxT(t), server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	var rejected *SessionError
	if !errors.As(err, &rejected) || rejected.Code != "validation-error" {
		t.Fatalf("got %v", err)
	}
}

// A routing miss before acceptance is retried as a new attempt: a new attempt
// id and new provisional references, the same idempotency key.
func TestARoutingMissStartsANewAttempt(t *testing.T) {
	var first received
	server := newFakeServer(t,
		func(c *fakeConn) {
			first = c.expect("invocationStart")
			c.send("invocationRejected", map[string]any{
				"attemptId": first.m["attemptId"], "code": "routing-miss", "message": "moving", "retryable": true,
			})
			c.waitClose()
		},
		func(c *fakeConn) {
			second := c.expect("invocationStart")
			if second.m["attemptId"] == first.m["attemptId"] || second.m["idempotencyKey"] != first.m["idempotencyKey"] {
				t.Errorf("retry %v after %v", second.m, first.m)
			}
			refs := provisionalRefs(t, second)
			if refs[0] == provisionalRefs(t, first)[0] {
				t.Error("a new attempt reused a provisional reference")
			}
			accept(c, second, "sess", inputMapping(1, refs[0], "in", 0, false))
			consumeInput(c, 1)
			c.send("invocationResult", map[string]any{"mappings": []any{}, "result": map[string]any{"kind": "none"}})
			finished(c)
			c.waitClose()
		},
	)
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consume",
		streamParams(EncodeStream(EncodeU32)(StreamOf[uint32](1)))); err != nil {
		t.Fatal(err)
	}
}

func TestAProducerFailureEndsTheStream(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		outputItem(c, 2, 0, "c0", 1)
		outputEnd(c, 2, 1, "c1", map[string]any{"kind": "error", "code": "producer-error", "message": "boom"})
		finished(c)
		c.waitClose()
	})
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	values, err := out.Collect(ctx)
	var failed *StreamError
	if !reflect.DeepEqual(values, []uint32{1}) || !errors.As(err, &failed) || failed.Message != "boom" {
		t.Fatalf("collected %v, %v", values, err)
	}
}

// Closing a returned stream early tells the server nobody reads it.
func TestClosingAStreamCancelsIt(t *testing.T) {
	cancelled := make(chan map[string]any, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		outputItem(c, 2, 0, "c0", 1)
		cancelled <- c.expect("streamCancel").m
		outputEnd(c, 2, 1, "", map[string]any{"kind": "cancelled", "reason": "consumer-drop"})
		finished(c)
		c.waitClose()
	})
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := out.Next(ctx); err != nil {
		t.Fatal(err)
	}
	if err := out.Close(); err != nil {
		t.Fatal(err)
	}
	m := <-cancelled
	if m["channel"] != float64(2) || m["reason"] != "consumer-drop" {
		t.Fatalf("cancelled with %v", m)
	}
}

// A failing input source is reported as unavailable, not as a clean end.
func TestAFailingSourceIsUnavailable(t *testing.T) {
	cancelled := make(chan map[string]any, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		c.expect("inputStreamItem")
		ack(c, 1, 1, false)
		cancelled <- c.expect("streamCancel").m
		c.send("invocationFinished", map[string]any{"outcome": map[string]any{
			"kind": "failure", "code": "invocation-failed", "message": "input unavailable",
		}})
		c.waitClose()
	})
	source := ProduceStream(func(w *AgentStreamWriter[uint32]) error {
		if err := w.Write(1); err != nil {
			return err
		}
		return errors.New("disk gone")
	})
	err := InvokeStreaming(ctxT(t), server.agent(t), "consume", streamParams(EncodeStream(EncodeU32)(source)))
	var failed *SessionError
	if !errors.As(err, &failed) || failed.Code != "invocation-failed" {
		t.Fatalf("got %v", err)
	}
	if m := <-cancelled; m["reason"] != "source-unavailable" {
		t.Fatalf("cancelled with %v", m)
	}
}

// A caller that gives up before the result cancels the streams it sent.
func TestGivingUpCancelsTheCall(t *testing.T) {
	cancelled := make(chan map[string]any, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		for {
			r := c.read()
			if r.kind == "streamCancel" {
				cancelled <- r.m
				return
			}
			if r.kind == "" {
				return
			}
		}
	})
	w, s := NewAgentStream[uint32]()
	defer func() { _ = w.Close() }()
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(200 * time.Millisecond)
		cancel()
	}()
	err := InvokeStreaming(ctx, server.agent(t), "consume", streamParams(EncodeStream(EncodeU32)(s)))
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
	select {
	case m := <-cancelled:
		if m["reason"] != "cancelled" {
			t.Fatalf("cancelled with %v", m)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the server never heard the cancellation")
	}
}

func TestStreamOfLeavesTheCallersSliceAlone(t *testing.T) {
	items := []uint32{1, 2}
	if _, err := StreamOf(items...).Collect(context.Background()); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(items, []uint32{1, 2}) {
		t.Fatalf("the caller's slice became %v", items)
	}
}

func TestAStreamIsReadOnce(t *testing.T) {
	s := StreamOf[uint32](1)
	if _, _, err := s.Next(context.Background()); err != nil {
		t.Fatal(err)
	}
	encoded := EncodeStream(EncodeU32)(s)
	if h := encoded.(schema.StreamValue).Handle.(*inputHandle); !errors.Is(h.err, ErrStreamConsumed) {
		t.Fatalf("a read stream was given away: %v", h.err)
	}
	fresh := StreamOf[uint32](1)
	EncodeStream(EncodeU32)(fresh)
	if _, _, err := fresh.Next(context.Background()); !errors.Is(err, ErrStreamConsumed) {
		t.Fatalf("a given stream was read: %v", err)
	}
}

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
	"net"
	"net/http"
	"net/http/httptest"
	"reflect"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// A scripted session server: each connection runs the next script.
type fakeServer struct {
	t       *testing.T
	mu      sync.Mutex
	scripts []func(*fakeConn)
	srv     *httptest.Server
	// conns counts the connections the server accepted.
	conns int
	// refusals are the HTTP statuses the next upgrade requests get.
	refusals []int
}

func (f *fakeServer) connections() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.conns
}

func newFakeServer(t *testing.T, scripts ...func(*fakeConn)) *fakeServer {
	f := &fakeServer{t: t, scripts: scripts}
	f.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != sessionEndpoint || r.Header.Get("Authorization") != "Bearer token" {
			http.Error(w, "unexpected request", http.StatusBadRequest)
			return
		}
		f.mu.Lock()
		if len(f.refusals) > 0 {
			status := f.refusals[0]
			f.refusals = f.refusals[1:]
			f.mu.Unlock()
			http.Error(w, "refused", status)
			return
		}
		f.mu.Unlock()
		ws, err := websocket.Accept(w, r, &websocket.AcceptOptions{Subprotocols: []string{sessionSubprotocol}})
		if err != nil {
			return
		}
		f.mu.Lock()
		f.conns++
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
	return f.agentWith(t, nil)
}

// agentWithin is an agent of the server with the given reconnect window.
func (f *fakeServer) agentWithin(t *testing.T, window time.Duration) *Agent {
	return agentAt(t, f.srv.URL, window)
}

func agentAt(t *testing.T, url string, window time.Duration) *Agent {
	a, err := NewAgent("Target", schema.RecordValue{Fields: []schema.SchemaValue{schema.StringValue{Value: "t1"}}},
		WithConfiguration(Configuration{Server: Custom(url, "token"), AppName: "app", EnvName: "env", ReconnectWindow: window}))
	if err != nil {
		t.Fatal(err)
	}
	return a
}

// agentWith is an agent of the server that sends its requests with client.
func (f *fakeServer) agentWith(t *testing.T, client *http.Client) *Agent {
	a, err := NewAgent("Target", schema.RecordValue{Fields: []schema.SchemaValue{schema.StringValue{Value: "t1"}}},
		WithConfiguration(Configuration{Server: Custom(f.srv.URL, "token"), AppName: "app", EnvName: "env", HTTPClient: client}))
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

// answerPings makes the server answer pings while the script only writes, as
// the real server does; the script reads nothing more, and waits on the
// returned context instead of waitClose.
func (c *fakeConn) answerPings() context.Context {
	return c.ws.CloseRead(context.Background())
}

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
	// The client sends input items only once it holds the acceptance, so the
	// first item tells the test that giving up now must cancel the stream.
	streaming := make(chan struct{})
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		first := true
		for {
			r := c.read()
			switch r.kind {
			case "inputStreamItem":
				if first {
					first = false
					close(streaming)
				}
			case "streamCancel":
				cancelled <- r.m
				return
			case "":
				return
			}
		}
	})
	w, s := NewAgentStream[uint32]()
	defer func() { _ = w.Close() }()
	if err := w.Write(1); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		select {
		case <-streaming:
		case <-time.After(10 * time.Second):
		}
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
	server := newFakeServer(t)
	s := StreamOf[uint32](1)
	if _, _, err := s.Next(context.Background()); err != nil {
		t.Fatal(err)
	}
	err := InvokeStreaming(ctxT(t), server.agent(t), "consume", streamParams(EncodeStream(EncodeU32)(s)))
	if !errors.Is(err, ErrStreamConsumed) {
		t.Fatalf("a read stream was given away: %v", err)
	}
	if n := server.connections(); n != 0 {
		t.Fatalf("a call with a read stream connected %d times", n)
	}
}

// Encoding a stream argument does not give it away: a call that fails to
// encode a later argument leaves the stream with its caller, who can pass it
// again.
func TestAStreamOfACallThatFailedToEncodeStaysWithTheCaller(t *testing.T) {
	items := make(chan []any, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		items <- consumeInput(c, 1)
		noResult(c)
		finished(c)
		c.waitClose()
	})
	produced := make(chan error, 1)
	s := ProduceStream(func(w *AgentStreamWriter[uint32]) error {
		err := w.WriteAll(1, 2, 3)
		produced <- err
		return err
	})
	params := func(q values.Quantity[meters]) (schema.SchemaValue, error) {
		return Encode(func() schema.SchemaValue {
			return schema.RecordValue{Fields: []schema.SchemaValue{EncodeStream(EncodeU32)(s), EncodeQuantity(q)}}
		})
	}
	ctx := ctxT(t)
	if _, err := params(values.Quantity[meters]{Mantissa: 1, Unit: "mi"}); err == nil {
		t.Fatal("a unit the quantity does not accept was encoded")
	} else if ee := (*EncodeError)(nil); !errors.As(err, &ee) || !strings.Contains(err.Error(), `unit "mi"`) {
		t.Fatalf("got %v, want an *EncodeError naming the unit", err)
	}
	good, err := params(values.Quantity[meters]{Mantissa: 1, Unit: "km"})
	if err != nil {
		t.Fatal(err)
	}
	if err := InvokeStreaming(ctx, server.agent(t), "calibrate", good); err != nil {
		t.Fatal(err)
	}
	if got := <-items; !reflect.DeepEqual(got, []any{float64(1), float64(2), float64(3)}) {
		t.Fatalf("the server read %v", got)
	}
	select {
	case err := <-produced:
		if err != nil {
			t.Fatalf("the producer failed: %v", err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the producer never finished")
	}
	if n := server.connections(); n != 1 {
		t.Fatalf("connected %d times", n)
	}
}

// An item its schema does not accept fails the call with the encode error;
// the items before it were sent, and the input is cancelled.
func TestAnItemItsSchemaRefusesFailsTheCall(t *testing.T) {
	seen := make(chan []string, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		var kinds []string
		for {
			_, data, err := c.ws.Read(context.Background())
			if err != nil {
				break
			}
			var r received
			if err := json.Unmarshal(data, &r.m); err != nil {
				t.Errorf("client sent bad JSON: %v", err)
				break
			}
			r.kind, _ = r.m["type"].(string)
			kinds = append(kinds, r.kind)
			switch r.kind {
			case "inputStreamItem":
				if v := r.m["value"].(map[string]any)["value"].(map[string]any); v["unit"] != "m" {
					t.Errorf("the server got %v", v)
				}
				ack(c, 1, 1, false)
			case "streamCancel":
				if r.m["reason"] != "source-unavailable" {
					t.Errorf("cancelled with %v", r.m)
				}
			}
		}
		seen <- kinds
	})
	s := StreamOf(
		values.Quantity[meters]{Mantissa: 1, Unit: "m"},
		values.Quantity[meters]{Mantissa: 2, Unit: "mi"},
		values.Quantity[meters]{Mantissa: 3, Unit: "m"},
	)
	err := InvokeStreaming(ctxT(t), server.agent(t), "measure", streamParams(EncodeStream(EncodeQuantity[meters])(s)))
	var ee *EncodeError
	if !errors.As(err, &ee) || !strings.Contains(err.Error(), `unit "mi"`) {
		t.Fatalf("got %v, want an *EncodeError naming the unit", err)
	}
	select {
	case kinds := <-seen:
		if !reflect.DeepEqual(kinds, []string{"inputStreamItem", "streamCancel"}) {
			t.Fatalf("the server got %v", kinds)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the failed call kept its connection")
	}
}

func noResult(c *fakeConn) {
	c.send("invocationResult", map[string]any{"mappings": []any{}, "result": map[string]any{"kind": "none"}})
}

// A frame the old connection had already received when the session gave it up
// is ignored, not a protocol error; one on the current connection before the
// acceptance still is.
func TestAFrameFromAStaleConnectionIsIgnored(t *testing.T) {
	item, err := encodeMessage("outputStreamItem", map[string]any{
		"channel": 2, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
		"value": map[string]any{"kind": "u32", "value": 1},
	})
	if err != nil {
		t.Fatal(err)
	}
	old, current := &sessionConn{}, &sessionConn{}
	s := &session{conn: current, accepted: true}
	if err := s.handleText(old, item); err != nil {
		t.Fatalf("a stale frame failed the session: %v", err)
	}
	s = &session{conn: current}
	if err := s.handleText(current, item); err == nil {
		t.Fatal("a frame before the acceptance was accepted")
	}
}

// A stream argument of a call that is never sent is stopped, so its writer is
// not left blocked.
func TestAStreamArgumentOfAFailedCallIsStopped(t *testing.T) {
	server := newFakeServer(t)
	w, s := NewAgentStream[uint32]()
	given := EncodeStream(EncodeU32)(s)
	again := EncodeStream(EncodeU32)(s)
	params := schema.RecordValue{Fields: []schema.SchemaValue{given, again}}
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consumeTwo", params); !errors.Is(err, ErrStreamConsumed) {
		t.Fatalf("got %v", err)
	}
	if n := server.connections(); n != 0 {
		t.Fatalf("a call with a stream passed twice connected %d times", n)
	}
	if _, _, err := s.Next(context.Background()); !errors.Is(err, ErrStreamConsumed) {
		t.Fatalf("a given stream was read: %v", err)
	}
	if err := w.Write(1); !errors.Is(err, ErrStreamClosed) {
		t.Fatalf("the writer of a stopped argument wrote: %v", err)
	}
}

// A call that finishes without reading all of an input stops it, so a writer
// waiting for room is released.
func TestAFinishedCallStopsItsUnreadInput(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		c.expect("inputStreamItem")
		noResult(c)
		finished(c)
		c.waitClose()
	})
	w, s := NewAgentStream[uint32]()
	if err := w.Write(1); err != nil {
		t.Fatal(err)
	}
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consumeSome", streamParams(EncodeStream(EncodeU32)(s))); err != nil {
		t.Fatal(err)
	}
	stopped := make(chan error, 1)
	go func() {
		for {
			if err := w.Write(1); err != nil {
				stopped <- err
				return
			}
		}
	}()
	select {
	case err := <-stopped:
		if !errors.Is(err, ErrStreamClosed) {
			t.Fatalf("the writer stopped with %v", err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the writer of an unread input was never released")
	}
}

// A partial acknowledgement keeps the rest of a packed batch in flight, and the
// acknowledgement of the rest releases it.
func TestAPartlyAcknowledgedBatchStaysInFlight(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		batch := c.expect("input-u8")
		if batch.binary == nil || batch.binary.sequence != 0 || len(batch.binary.payload) != 5 {
			t.Errorf("batch %+v", batch.binary)
		}
		ack(c, 1, 2, false)
		ack(c, 1, 5, false)
		if end := c.expect("inputStreamEnd"); end.m["sequence"] != "5" {
			t.Errorf("the end came at %v", end.m["sequence"])
		}
		ack(c, 1, 5, true)
		noResult(c)
		finished(c)
		c.waitClose()
	})
	params := streamParams(EncodeStream(EncodeU8)(StreamOf[uint8](1, 2, 3, 4, 5)))
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consumeBytes", params); err != nil {
		t.Fatal(err)
	}
}

// A stream inside an input item gets a provisional reference; the item's
// acknowledgement maps it to a channel, on which the nested stream flows.
func TestANestedInputStreamFlowsOnItsOwnChannel(t *testing.T) {
	var nested []any
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "outer", 0, false))
		item := c.expect("inputStreamItem")
		value := item.m["value"].(map[string]any)
		if value["kind"] != "stream" {
			t.Errorf("the item is %v", value)
			return
		}
		provisional := value["value"].(map[string]any)["provisionalRef"].(string)
		c.send("inputStreamAck", map[string]any{
			"channel": 1, "highestContiguousSequence": "1", "terminal": false,
			"mappings": []any{inputMapping(3, provisional, "nested", 0, false)},
		})
		outerEnded, nestedEnded := false, false
		for !outerEnded || !nestedEnded {
			r := c.read()
			switch {
			case r.kind == "inputStreamItem" && r.m["channel"] == float64(3):
				nested = append(nested, r.m["value"].(map[string]any)["value"])
				ack(c, 3, uint64(len(nested)), false)
			case r.kind == "inputStreamEnd" && r.m["channel"] == float64(1):
				ack(c, 1, 1, true)
				outerEnded = true
			case r.kind == "inputStreamEnd" && r.m["channel"] == float64(3):
				ack(c, 3, uint64(len(nested)), true)
				nestedEnded = true
			default:
				t.Errorf("unexpected %s %v", r.kind, r.m)
				return
			}
		}
		noResult(c)
		finished(c)
		c.waitClose()
	})
	outer := StreamOf(StreamOf[uint32](7, 8))
	params := streamParams(EncodeStream(EncodeStream(EncodeU32))(outer))
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consumeNested", params); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(nested, []any{float64(7), float64(8)}) {
		t.Fatalf("the nested stream carried %v", nested)
	}
}

const nestedU32StreamGraph = `{"root":{"kind":"stream","value":{"inner":{"kind":"stream","value":{"inner":{"kind":"u32","value":{}}}}}}}`

// An outer item that binds an inner stream and is dropped unread by a
// reconnect binds that stream again when the server replays it.
func TestAReplayedUnreadItemBindsItsInnerStreamAgain(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.send("outputStreamItem", map[string]any{
				"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(3, "inner")}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			if cursors, _ := resume.m["outputCursors"].([]any); len(cursors) != 0 {
				t.Errorf("resumed after a delivered item: %v", resume.m)
			}
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			outputEnd(c, 5, 1, "c1", map[string]any{"kind": "ok"})
			outputItem(c, 6, 0, "i0", 7)
			outputEnd(c, 6, 1, "i1", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	inner, ok, err := out.Next(ctx)
	if err != nil || !ok {
		t.Fatalf("outer item %v %v", ok, err)
	}
	values, err := inner.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{7}) {
		t.Fatalf("inner stream %v, %v", values, err)
	}
	if _, ok, err := out.Next(ctx); ok || err != nil {
		t.Fatalf("the outer stream did not end: %v %v", ok, err)
	}
}

// Closing a stream wakes a reader that waits on it with no deadline.
func TestClosingAStreamWakesItsWaitingReader(t *testing.T) {
	// The server holds the stream end back, so only Close can wake the reader.
	checked := make(chan struct{})
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		c.expect("streamCancel")
		<-checked
		outputEnd(c, 2, 0, "", map[string]any{"kind": "cancelled", "reason": "consumer-drop"})
		finished(c)
		c.waitClose()
	})
	out, err := CallStreaming(ctxT(t), server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	type next struct {
		ok  bool
		err error
	}
	done := make(chan next, 1)
	go func() {
		_, ok, err := out.Next(context.Background())
		done <- next{ok, err}
	}()
	time.Sleep(50 * time.Millisecond)
	if err := out.Close(); err != nil {
		t.Fatal(err)
	}
	select {
	case r := <-done:
		if r.ok || r.err != nil {
			t.Fatalf("Next after Close = %v, %v", r.ok, r.err)
		}
	case <-time.After(5 * time.Second):
		t.Error("Close did not wake the waiting reader")
	}
	close(checked)
}

// An item that waits for queue space when its connection goes binds its inner
// stream again when the server replays it on the next connection.
func TestAReplayedItemThatWaitedForSpaceBindsItsInnerStreamAgain(t *testing.T) {
	fastPing(t)
	const items = maxQueuedItems + 1
	inner := func(i int) string { return "inner" + itoa(uint64(i)) }
	resumed := make(chan struct{})
	var key any
	sendItem := func(c *fakeConn, channel uint32, i int, mappings []any) {
		c.send("outputStreamItem", map[string]any{
			"channel": channel, "cursorToken": "c" + itoa(uint64(i)), "mappings": mappings,
			"sequence": itoa(uint64(i)),
			"value":    map[string]any{"kind": "stream", "value": map[string]any{"streamToken": inner(i)}},
		})
	}
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			c.answerPings()
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			for i := range items {
				sendItem(c, 2, i, []any{outputMapping(uint32(10+i), inner(i))})
			}
			// The client reads no item, so the last one waits for space when
			// the connection drops.
			time.Sleep(100 * time.Millisecond)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			mappings := []map[string]any{outputMapping(5, "out")}
			for i := range items {
				mappings = append(mappings, outputMapping(uint32(1000+i), inner(i)))
			}
			closed := c.answerPings()
			acceptAs(c, resume, key, "sess2", mappings...)
			close(resumed)
			for i := range items {
				sendItem(c, 5, i, []any{})
			}
			outputEnd(c, 5, items, "end", map[string]any{"kind": "ok"})
			finished(c)
			<-closed.Done()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	for i := range items {
		if _, ok, err := out.Next(ctx); err != nil || !ok {
			t.Fatalf("item %d: %v %v", i, ok, err)
		}
	}
}

// A resume refused because the old attachment is still live is retried once
// as a takeover with the same cursors; the session then goes on.
func TestARefusedResumeIsRetriedAsATakeover(t *testing.T) {
	readFirst := make(chan struct{})
	var key any
	var takeover received
	refuse := func(c *fakeConn, op received) {
		c.send("invocationRejected", map[string]any{
			"attemptId": op.m["attemptId"], "code": "invalid-attachment-state",
			"message": "still attached", "retryable": false,
		})
		c.waitClose()
	}
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			<-readFirst
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			if resume.m["operation"] != "resume" {
				t.Errorf("first reattach is %v", resume.m)
			}
			refuse(c, resume)
		},
		func(c *fakeConn) {
			takeover = c.expect("resumeAttach")
			acceptAs(c, takeover, key, "sess2", outputMapping(6, "out"))
			outputItem(c, 6, 1, "c1", 20)
			outputEnd(c, 6, 2, "c2", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if first, ok, err := out.Next(ctx); err != nil || !ok || first != 10 {
		t.Fatalf("first item %v %v %v", first, ok, err)
	}
	close(readFirst)
	rest, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(rest, []uint32{20}) {
		t.Fatalf("rest %v, %v", rest, err)
	}
	if takeover.m["operation"] != "takeover" || !reflect.DeepEqual(takeover.m["outputCursors"], []any{"c0"}) {
		t.Fatalf("took over with %v", takeover.m)
	}
}

// A refused takeover ends the session: the fallback is tried once.
func TestARefusedTakeoverEndsTheSession(t *testing.T) {
	readFirst := make(chan struct{})
	refuse := func(c *fakeConn) {
		op := c.expect("resumeAttach")
		c.send("invocationRejected", map[string]any{
			"attemptId": op.m["attemptId"], "code": "invalid-attachment-state",
			"message": "still attached", "retryable": false,
		})
		c.waitClose()
	}
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			<-readFirst
			c.drop()
		},
		refuse,
		refuse,
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "produce", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := out.Next(ctx); err != nil {
		t.Fatal(err)
	}
	close(readFirst)
	_, err = out.Collect(ctx)
	var refused *SessionError
	if !errors.As(err, &refused) || refused.Code != "invalid-attachment-state" {
		t.Fatalf("got %v", err)
	}
}

// The schedule a slow reader allows: the old connection's reader binds an
// item's inner stream and pauses before it queues the item, the session moves
// to a new connection, and the new reader gets the replayed item first. The
// replay binds the stream, and the old reader's late offer leaves it bound.
func TestAReplayedItemBindsWhileTheOldReaderIsPaused(t *testing.T) {
	old, current := &sessionConn{}, &sessionConn{}
	s := &session{
		conn: old, accepted: true, everAccepted: true, sessionToken: "sess",
		byProvisional: map[string]*inputStream{}, byToken: map[string]*inputStream{},
		outputs: map[string]*outputStream{}, inflight: map[uint64]schema.SchemaValue{},
		directions: map[string]string{}, channels: map[uint32]string{},
		tokenChannels: map[string]uint32{}, cursors: map[string]string{},
		result: make(chan sessionResult, 1), done: make(chan struct{}),
	}
	frame := func(kind string, members map[string]any) []byte {
		data, err := encodeMessage(kind, members)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	item := func(channel uint32, mappings []any) []byte {
		return frame("outputStreamItem", map[string]any{
			"channel": channel, "cursorToken": "c0", "mappings": mappings, "sequence": "0",
			"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
		})
	}
	if err := s.handleText(old, frame("invocationResult", map[string]any{
		"mappings": []any{outputMapping(2, "out")},
		"result": map[string]any{
			"graph": json.RawMessage(nestedU32StreamGraph), "kind": "value",
			"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "out"}},
		},
	})); err != nil {
		t.Fatal(err)
	}

	_, m, err := parseMessage(item(2, []any{outputMapping(3, "inner")}))
	if err != nil {
		t.Fatal(err)
	}
	out, paused, err := s.bindItem(old, m)
	if err != nil {
		t.Fatal(err)
	}

	s.mu.Lock()
	if err := s.detach(old); err != nil {
		t.Fatal(err)
	}
	s.conn, s.accepted = current, true
	s.mu.Unlock()
	if err := s.handleText(current, item(5, []any{outputMapping(5, "out"), outputMapping(6, "inner")})); err != nil {
		t.Fatalf("the replayed item failed: %v", err)
	}

	if err := out.offer(old, paused); err != nil {
		t.Fatal(err)
	}
	if !s.outputs["inner"].bound {
		t.Fatal("the old reader's late offer unbound the replayed stream")
	}
}

// An item that arrives on a connection the session has left binds nothing,
// so its replay on the next connection binds its streams.
func TestAnItemOnALeftConnectionBindsNothing(t *testing.T) {
	old, current := &sessionConn{}, &sessionConn{}
	s := &session{
		conn: current, accepted: true, everAccepted: true, sessionToken: "sess",
		byProvisional: map[string]*inputStream{}, byToken: map[string]*inputStream{},
		outputs: map[string]*outputStream{}, inflight: map[uint64]schema.SchemaValue{},
		directions: map[string]string{}, channels: map[uint32]string{},
		tokenChannels: map[string]uint32{}, cursors: map[string]string{},
		result: make(chan sessionResult, 1), done: make(chan struct{}),
	}
	data, err := encodeMessage("outputStreamItem", map[string]any{
		"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(2, "out"), outputMapping(3, "inner")},
		"sequence": "0",
		"value":    map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	_, m, err := parseMessage(data)
	if err != nil {
		t.Fatal(err)
	}
	out, _, err := s.bindItem(old, m)
	if err != nil || out != nil {
		t.Fatalf("a stale item was taken: %v %v", out, err)
	}
	if len(s.channels) != 0 || len(s.inflight) != 0 {
		t.Fatalf("a stale item changed the session: %v %v", s.channels, s.inflight)
	}
}

// On resume the server pumps the outer and inner streams concurrently, so the
// inner stream's first item can arrive before the replayed outer item that
// carries it; the inner stream keeps its item type and takes the item.
func TestAnInnerItemMayArriveBeforeItsOuterItemIsReplayed(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.send("outputStreamItem", map[string]any{
				"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(3, "inner")}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			time.Sleep(100 * time.Millisecond)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			outputItem(c, 6, 0, "i0", 7)
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			outputEnd(c, 5, 1, "c1", map[string]any{"kind": "ok"})
			outputEnd(c, 6, 1, "i1", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	inner, ok, err := out.Next(ctx)
	if err != nil || !ok {
		t.Fatalf("outer item %v %v", ok, err)
	}
	values, err := inner.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{7}) {
		t.Fatalf("inner stream %v, %v", values, err)
	}
}

// The reader closes the outer stream while an unread item holds an inner
// stream, and the connection drops before the end: the server's replay of the
// item binds the inner stream again rather than failing the session.
func TestAReplayAfterCloseDoesNotFailTheSession(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.send("outputStreamItem", map[string]any{
				"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(3, "inner")}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.expect("streamCancel")
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	s := out.core.src.(*remoteSource[AgentStream[uint32]]).out.s
	time.Sleep(100 * time.Millisecond)
	_ = out.Close()
	<-resumed
	time.Sleep(300 * time.Millisecond)
	s.mu.Lock()
	err = s.err
	s.mu.Unlock()
	if err != nil && strings.Contains(err.Error(), "twice") {
		t.Fatal("replay after Close failed the session")
	}
}

// Closing the outer stream stops the inner stream an unread item carried, so
// that stream's items do not fill a queue nobody reads and stall the session.
func TestClosingAStreamStopsTheStreamsItsUnreadItemsCarried(t *testing.T) {
	gotEnd := make(chan struct{})
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.send("outputStreamItem", map[string]any{
				"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(3, "inner")}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.expect("streamCancel")
			for i := 0; i < maxQueuedItems+2; i++ {
				outputItem(c, 3, uint64(i), "i"+itoa(uint64(i)), uint32(i))
			}
			outputEnd(c, 2, 1, "", map[string]any{"kind": "cancelled", "reason": "consumer-drop"})
			close(gotEnd)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	o := out.core.src.(*remoteSource[AgentStream[uint32]]).out
	time.Sleep(100 * time.Millisecond)
	_ = out.Close()
	<-gotEnd
	time.Sleep(500 * time.Millisecond)
	o.s.mu.Lock()
	terminal := o.terminal
	innerQ := len(o.s.outputs["inner"].queue)
	innerExposed := o.s.outputs["inner"].exposed
	o.s.mu.Unlock()
	if !terminal || innerQ != 0 || innerExposed {
		t.Fatalf("outer terminal=%v, inner queued=%d, exposed=%v", terminal, innerQ, innerExposed)
	}
}

// A resume can announce a stream the client has never seen returned, and the
// server can send that stream's items before the value that returns it: the
// stream holds them until the value gives it its item type.
func TestAStreamHoldsItsItemsUntilAValueReturnsIt(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			outputItem(c, 6, 0, "i0", 7)
			outputItem(c, 6, 1, "i1", 8)
			outputEnd(c, 6, 2, "i2", map[string]any{"kind": "ok"})
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			outputEnd(c, 5, 1, "c1", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	inner, ok, err := out.Next(ctx)
	if err != nil || !ok {
		t.Fatalf("outer item %v %v", ok, err)
	}
	values, err := inner.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{7, 8}) {
		t.Fatalf("inner stream %v, %v", values, err)
	}
	if _, ok, err := out.Next(ctx); ok || err != nil {
		t.Fatalf("the outer stream did not end: %v %v", ok, err)
	}
}

// A stream that is never returned cannot make the session hold frames
// without bound: the held bytes have a session-wide budget.
func TestHeldFramesHaveABudget(t *testing.T) {
	s := &session{}
	o := &outputStream{s: s}
	if err := o.hold(heldFrame{bytes: maxHeldBytes}); err != nil {
		t.Fatal(err)
	}
	if err := o.hold(heldFrame{bytes: 1}); err == nil || !strings.Contains(err.Error(), "never returned") {
		t.Fatalf("got %v", err)
	}
	o.dropHeld()
	if s.heldBytes != 0 {
		t.Fatalf("held bytes after a drop: %d", s.heldBytes)
	}
}

// The reader closes the outer stream before a resume. The inner stream's items
// are held, and the replayed outer item types the inner stream while the outer
// one is stopped: the held items keep the inner stream's sequence in step.
func TestHeldFramesOfAStreamAStoppedStreamTypesKeepItsSequence(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.expect("streamCancel")
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			outputItem(c, 6, 0, "i0", 7)
			outputItem(c, 6, 1, "i1", 8)
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			outputItem(c, 6, 2, "i2", 9)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	s := out.core.src.(*remoteSource[AgentStream[uint32]]).out.s
	_ = out.Close()
	<-resumed
	time.Sleep(300 * time.Millisecond)
	s.mu.Lock()
	err = s.err
	s.mu.Unlock()
	if err != nil {
		t.Fatalf("session failed: %v", err)
	}
}

// A replayed backlog longer than a stream's queue may arrive before the outer
// item that returns the stream.
func TestALongBacklogMayArriveBeforeTheItemThatReturnsItsStream(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			time.Sleep(100 * time.Millisecond)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			for i := range maxQueuedItems + 1 {
				outputItem(c, 6, uint64(i), "i"+itoa(uint64(i)), 1)
			}
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	_, _, err = out.Next(ctx)
	if err != nil {
		t.Fatalf("outer item: %v", err)
	}
}

// The reader of a connection the session has left does not take the frames the
// current connection's reader is about to release.
func TestAStaleReaderLeavesTheReleaseListAlone(t *testing.T) {
	s := &session{}
	c1, c2 := &sessionConn{}, &sessionConn{}
	x := &outputStream{s: s, held: []heldFrame{{binary: []byte{1}}}, typed: true}
	s.conn = c2
	s.boundNow = []*outputStream{x}
	if err := s.releaseHeld(c1); err != nil {
		t.Fatal(err)
	}
	if len(s.boundNow) == 0 {
		t.Fatalf("a stale reader cleared the release list; x still holds %d frames", len(x.held))
	}
}

// A stream no value ever returned does not keep a finished invocation from
// finishing: it was never the caller's, and its held frames are dropped.
func TestAStreamNeverReturnedDoesNotKeepTheInvocationOpen(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.expect("streamCancel")
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			outputItem(c, 6, 0, "i0", 7)
			outputEnd(c, 6, 1, "i1", map[string]any{"kind": "ok"})
			outputEnd(c, 5, 0, "", map[string]any{"kind": "cancelled", "reason": "consumer-drop"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	s := out.core.src.(*remoteSource[AgentStream[uint32]]).out.s
	_ = out.Close()
	<-resumed
	time.Sleep(300 * time.Millisecond)
	s.mu.Lock()
	err = s.err
	s.mu.Unlock()
	if err != nil {
		t.Fatalf("session failed: %v", err)
	}
}

// A stream returned on the first connection by an item still unread at the
// drop is unreachable after the resume until the item is replayed: its
// backlog, longer than a queue, is held rather than blocking the read loop.
func TestAnUnreachableStreamHoldsItsBacklogUntilItsItemIsReplayed(t *testing.T) {
	resumed := make(chan struct{})
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", nestedU32StreamGraph)
			c.send("outputStreamItem", map[string]any{
				"channel": 2, "cursorToken": "c0", "mappings": []any{outputMapping(3, "inner")}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			time.Sleep(200 * time.Millisecond)
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(5, "out"), outputMapping(6, "inner"))
			close(resumed)
			for i := range maxQueuedItems + 1 {
				outputItem(c, 6, uint64(i), "i"+itoa(uint64(i)), 1)
			}
			c.send("outputStreamItem", map[string]any{
				"channel": 5, "cursorToken": "c0", "mappings": []any{}, "sequence": "0",
				"value": map[string]any{"kind": "stream", "value": map[string]any{"streamToken": "inner"}},
			})
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "nested", schema.RecordValue{},
		DecodeStream(DecodeStream(DecodeU32)))
	if err != nil {
		t.Fatal(err)
	}
	<-resumed
	_, _, err = out.Next(ctx)
	if err != nil {
		t.Fatalf("outer item: %v", err)
	}
}

// fastPing makes the keepalive ping of the sessions the test opens fire often
// and time out sooner. A pong is read after the frames before it, so the
// timeout still leaves the client room to read a burst of items.
func fastPing(t *testing.T) {
	interval, timeout := pingInterval, pingTimeout
	pingInterval, pingTimeout = 20*time.Millisecond, 300*time.Millisecond
	t.Cleanup(func() { pingInterval, pingTimeout = interval, timeout })
}

// A reader slower than the server parks the read loop on a full queue; the
// connection stays, although no pong can be read while the loop waits.
func TestASlowReaderKeepsItsConnection(t *testing.T) {
	fastPing(t)
	items := maxQueuedItems + 4
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		closed := c.answerPings()
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		for i := range items {
			outputItem(c, 2, uint64(i), "c"+itoa(uint64(i)), uint32(i))
		}
		outputEnd(c, 2, uint64(items), "end", map[string]any{"kind": "ok"})
		finished(c)
		<-closed.Done()
	})
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agent(t), "numbers", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(4 * pingTimeout)
	if n := server.connections(); n != 1 {
		t.Fatalf("the session opened %d connections while the reader was slow", n)
	}
	values, err := out.Collect(ctx)
	if err != nil || len(values) != items {
		t.Fatalf("collected %d values, %v", len(values), err)
	}
}

// stallingConn is a client connection that blocks every write of at least
// stallBytes while stall is set, until the connection closes. Ping frames are
// smaller, so they still go through.
type stallingConn struct {
	net.Conn
	stall  *atomic.Bool
	closed chan struct{}
	once   sync.Once
}

const stallBytes = 4096

func (c *stallingConn) Write(b []byte) (int, error) {
	if len(b) >= stallBytes && c.stall.Load() {
		<-c.closed
		return 0, net.ErrClosed
	}
	return c.Conn.Write(b)
}

func (c *stallingConn) Close() error {
	c.once.Do(func() { close(c.closed) })
	return c.Conn.Close()
}

func stallingClient(stall *atomic.Bool) *http.Client {
	var dialer net.Dialer
	return &http.Client{Transport: &http.Transport{
		DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			conn, err := dialer.DialContext(ctx, network, addr)
			if err != nil {
				return nil, err
			}
			return &stallingConn{Conn: conn, stall: stall, closed: make(chan struct{})}, nil
		},
	}}
}

// A frame write that never finishes holds the frame-write lock, so no ping
// is written either. The connection is lost although the read loop is
// parked on a full queue.
func TestAStalledWriteIsALostConnectionWhileTheReaderIsParked(t *testing.T) {
	fastPing(t)
	var stall atomic.Bool
	filled := make(chan struct{})
	reconnected := make(chan struct{})
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			refs := provisionalRefs(t, start)
			accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
			streamResult(c, 2, "out", u32StreamGraph)
			for i := range maxQueuedItems + 4 {
				outputItem(c, 2, uint64(i), "c"+itoa(uint64(i)), uint32(i))
			}
			close(filled)
			<-c.answerPings().Done()
		},
		func(c *fakeConn) {
			c.expect("resumeAttach")
			close(reconnected)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	w, in := NewAgentStream[string]()
	out, err := CallStreaming(ctx, server.agentWith(t, stallingClient(&stall)), "transform",
		streamParams(EncodeStream(EncodeString)(in)), DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = out.Close() }()
	<-filled
	time.Sleep(2 * pingTimeout)
	stall.Store(true)
	if err := w.Write(strings.Repeat("x", 4*stallBytes)); err != nil {
		t.Fatal(err)
	}
	select {
	case <-reconnected:
	case <-time.After(10 * pingTimeout):
		t.Fatalf("the session kept a connection whose write stalled; %d connections", server.connections())
	}
}

// A server that accepts a connection and closes it at once is dialed again
// with a growing delay, not in a tight loop.
func TestReconnectsAfterAnImmediateCloseBackOff(t *testing.T) {
	drop := func(c *fakeConn) {
		c.expect("invocationStart")
		c.drop()
	}
	server := newFakeServer(t, drop, drop, drop, drop, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		outputEnd(c, 2, 0, "end", map[string]any{"kind": "ok"})
		finished(c)
		c.waitClose()
	})
	ctx := ctxT(t)
	started := time.Now()
	out, err := CallStreaming(ctx, server.agent(t), "numbers", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := out.Collect(ctx); err != nil {
		t.Fatal(err)
	}
	// The four reconnects wait 50, 100, 200 and 400 ms.
	if elapsed := time.Since(started); elapsed < 700*time.Millisecond {
		t.Fatalf("five connections took only %v", elapsed)
	}
}

// faultyItemCall runs a call whose one stream argument yields an item the
// call cannot send, and returns what the server got after accepting it and
// the call's error.
func faultyItemCall(t *testing.T, arg schema.SchemaValue) ([]string, error) {
	t.Helper()
	seen := make(chan []string, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		var got []string
		for {
			r := c.readUntilClose()
			if r.kind == "" {
				break
			}
			got = append(got, r.kind)
			if r.kind == "streamCancel" && r.m["reason"] != "source-unavailable" {
				t.Errorf("cancelled with %v", r.m)
			}
		}
		seen <- got
	})
	err := InvokeStreaming(ctxT(t), server.agent(t), "consume", streamParams(arg))
	select {
	case got := <-seen:
		return got, err
	case <-time.After(30 * time.Second):
		t.Fatal("the failed call kept its connection")
		return nil, nil
	}
}

// readUntilClose reads the next message, or nothing once the client closed
// the connection.
func (c *fakeConn) readUntilClose() received {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	kind, data, err := c.ws.Read(ctx)
	if err != nil {
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

// An item the call cannot send fails the call with a StreamItemError; the
// input is cancelled as unavailable and nothing of the item is sent.
func TestAnItemTheCallCannotSendFailsTheCall(t *testing.T) {
	consumed := StreamOf[uint32](1)
	if _, _, err := consumed.Next(context.Background()); err != nil {
		t.Fatal(err)
	}
	notAMediaType := "png"
	for _, tc := range []struct {
		name    string
		arg     schema.SchemaValue
		message string
		is      error
	}{
		{
			name: "a u8 lane item that is not a u8",
			arg: EncodeStream(func(uint8) schema.SchemaValue { return schema.StringValue{Value: "x"} })(
				StreamOf[uint8](1)),
			message: "expected a u8 value",
		},
		{
			name: "a binary lane item that is not binary",
			arg: EncodeStream(func(values.Binary) schema.SchemaValue { return schema.U8Value{Value: 1} })(
				StreamOf(values.Binary{1})),
			message: "expected a binary value",
		},
		{
			name:    "a binary item over the size limit",
			arg:     EncodeStream(EncodeBinary)(StreamOf(make(values.Binary, maxBinaryBytes+1))),
			message: "exceeds 16 MiB",
		},
		{
			name: "a binary item whose media type is not one",
			arg: EncodeStream(func(v values.Binary) schema.SchemaValue {
				return schema.BinaryValue{Bytes: v, MimeType: &notAMediaType}
			})(StreamOf(values.Binary{1})),
			message: `"png" is not a MIME type`,
		},
		{
			name:    "a JSON item over the size limit",
			arg:     EncodeStream(EncodeString)(StreamOf(strings.Repeat("\x01", maxBinaryBytes/6+1))),
			message: "exceeds 16 MiB",
		},
		{
			name:    "an item holding a stream that was already read",
			arg:     EncodeStream(EncodeStream(EncodeU32))(StreamOf(consumed)),
			message: "",
			is:      ErrStreamConsumed,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, err := faultyItemCall(t, tc.arg)
			var unsendable *StreamItemError
			if !errors.As(err, &unsendable) || !strings.Contains(err.Error(), tc.message) {
				t.Fatalf("got %v, want a *StreamItemError saying %q", err, tc.message)
			}
			if tc.is != nil && !errors.Is(err, tc.is) {
				t.Fatalf("got %v, want one that is %v", err, tc.is)
			}
			if !reflect.DeepEqual(got, []string{"streamCancel"}) {
				t.Fatalf("the server got %v", got)
			}
		})
	}
}

// A source that fails only makes its input unavailable: the server decides
// what that means for the call, and here it still completes it.
func TestAFailingSourceDoesNotFailTheCall(t *testing.T) {
	cancelled := make(chan map[string]any, 1)
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		refs := provisionalRefs(t, start)
		accept(c, start, "sess", inputMapping(1, refs[0], "in", 0, false))
		c.expect("inputStreamItem")
		ack(c, 1, 1, false)
		cancelled <- c.expect("streamCancel").m
		noResult(c)
		finished(c)
		c.waitClose()
	})
	source := ProduceStream(func(w *AgentStreamWriter[uint32]) error {
		if err := w.Write(1); err != nil {
			return err
		}
		return errors.New("disk gone")
	})
	if err := InvokeStreaming(ctxT(t), server.agent(t), "consume", streamParams(EncodeStream(EncodeU32)(source))); err != nil {
		t.Fatalf("a failing source failed the call: %v", err)
	}
	if m := <-cancelled; m["reason"] != "source-unavailable" {
		t.Fatalf("cancelled with %v", m)
	}
}

// A server that answers nothing makes the start fail once the window passes.
func TestAStartAgainstAClosedPortIsUnreachable(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	url := "http://" + listener.Addr().String()
	_ = listener.Close()
	started := time.Now()
	_, err = CallStreaming(ctxT(t), agentAt(t, url, 300*time.Millisecond), "numbers",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	elapsed := time.Since(started)
	var unreachable *UnreachableError
	if !errors.As(err, &unreachable) {
		t.Fatalf("got %v", err)
	}
	if unreachable.Window != 300*time.Millisecond || !strings.HasPrefix(unreachable.Endpoint, "ws://"+listener.Addr().String()) {
		t.Fatalf("unreachable %+v", unreachable)
	}
	var cause *Error
	if !errors.As(err, &cause) || cause.Status != 0 {
		t.Fatalf("cause %v", unreachable.Cause)
	}
	if elapsed < 300*time.Millisecond || elapsed > 3*time.Second {
		t.Fatalf("gave up after %v", elapsed)
	}
}

// A peer that accepts the TCP connection but never answers the upgrade is
// abandoned after the connect timeout, and the next attempt dials again.
func TestAnUpgradeThatNeverCompletesIsAbandoned(t *testing.T) {
	timeout := connectTimeout
	connectTimeout = 100 * time.Millisecond
	t.Cleanup(func() { connectTimeout = timeout })
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	var accepted atomic.Int32
	go func() {
		for {
			c, err := listener.Accept()
			if err != nil {
				return
			}
			accepted.Add(1)
			t.Cleanup(func() { _ = c.Close() })
		}
	}()
	_, err = CallStreaming(ctxT(t), agentAt(t, "http://"+listener.Addr().String(), time.Second), "numbers",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	var unreachable *UnreachableError
	if !errors.As(err, &unreachable) || !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("got %v", err)
	}
	if n := accepted.Load(); n < 3 {
		t.Fatalf("dialed %d times", n)
	}
}

// 429 and 5xx on the upgrade are retried; the call goes on once the server
// upgrades.
func TestARetryableUpgradeStatusIsRetried(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		outputEnd(c, 2, 0, "end", map[string]any{"kind": "ok"})
		finished(c)
		c.waitClose()
	})
	server.refusals = []int{http.StatusServiceUnavailable, http.StatusTooManyRequests, http.StatusServiceUnavailable}
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agentWithin(t, 5*time.Second), "numbers", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := out.Collect(ctx); err != nil {
		t.Fatal(err)
	}
}

// Another HTTP status on the upgrade fails the call at once.
func TestARefusedUpgradeFailsAtOnce(t *testing.T) {
	server := newFakeServer(t)
	server.refusals = []int{http.StatusForbidden}
	_, err := CallStreaming(ctxT(t), server.agent(t), "numbers", schema.RecordValue{}, DecodeStream(DecodeU32))
	var failure *Error
	var unreachable *UnreachableError
	if !errors.As(err, &failure) || failure.Status != http.StatusForbidden || errors.As(err, &unreachable) {
		t.Fatalf("got %v", err)
	}
}

// Retryable upgrade statuses past the window end the call with the last one.
func TestRetryableStatusesPastTheWindowAreUnreachable(t *testing.T) {
	server := newFakeServer(t)
	server.refusals = slices.Repeat([]int{http.StatusBadGateway}, 100)
	_, err := CallStreaming(ctxT(t), server.agentWithin(t, 300*time.Millisecond), "numbers",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	var unreachable *UnreachableError
	var failure *Error
	if !errors.As(err, &unreachable) || !errors.As(err, &failure) || failure.Status != http.StatusBadGateway {
		t.Fatalf("got %v", err)
	}
}

// Retryable rejections are retried within the window and do not reset it.
func TestRetryableRejectionsPastTheWindowAreUnreachable(t *testing.T) {
	reject := func(c *fakeConn) {
		start := c.expect("invocationStart")
		c.send("invocationRejected", map[string]any{
			"attemptId": start.m["attemptId"], "code": "routing-miss", "message": "moving", "retryable": true,
		})
		c.waitClose()
	}
	server := newFakeServer(t, slices.Repeat([]func(*fakeConn){reject}, 100)...)
	_, err := CallStreaming(ctxT(t), server.agentWithin(t, 400*time.Millisecond), "numbers",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	var unreachable *UnreachableError
	var rejected *SessionError
	if !errors.As(err, &unreachable) || !errors.As(err, &rejected) || rejected.Code != "routing-miss" {
		t.Fatalf("got %v", err)
	}
	if n := server.connections(); n < 2 {
		t.Fatalf("connected %d times", n)
	}
}

// stableAfter sets how long an accepted connection must stay up to count as
// progress for the rest of the test.
func stableAfter(t *testing.T, d time.Duration) {
	before := stableConnection
	stableConnection = d
	t.Cleanup(func() { stableConnection = before })
}

// The window runs from the loss of a stable connection, not from the start:
// a call accepted for longer than the window still resumes.
func TestTheWindowStartsAgainAfterAcceptance(t *testing.T) {
	stableAfter(t, 200*time.Millisecond)
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			time.Sleep(500 * time.Millisecond)
			c.drop()
		},
		func(c *fakeConn) {
			c.expect("resumeAttach")
			c.drop()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, key, "sess2", outputMapping(6, "out"))
			outputItem(c, 6, 1, "c1", 20)
			outputEnd(c, 6, 2, "c2", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agentWithin(t, 300*time.Millisecond), "produce",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	values, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{10, 20}) {
		t.Fatalf("collected %v, %v", values, err)
	}
}

// Without a limit the call keeps reconnecting.
func TestNoReconnectLimitKeepsRetrying(t *testing.T) {
	server := newFakeServer(t, func(c *fakeConn) {
		start := c.expect("invocationStart")
		accept(c, start, "sess")
		streamResult(c, 2, "out", u32StreamGraph)
		outputEnd(c, 2, 0, "end", map[string]any{"kind": "ok"})
		finished(c)
		c.waitClose()
	})
	server.refusals = slices.Repeat([]int{http.StatusServiceUnavailable}, 5)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agentWithin(t, NoReconnectLimit), "numbers", schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := out.Collect(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestANegativeReconnectWindowIsInvalid(t *testing.T) {
	err := Configure(Configuration{Server: Custom("http://localhost", "token"), AppName: "app", EnvName: "env", ReconnectWindow: -time.Second})
	if err == nil {
		t.Fatal("accepted a negative window")
	}
}

// A retryable rejection of the accepted attempt resumes the call, with a
// window that starts when the call leaves the stable connection.
func TestARetryableRejectionAfterAcceptanceResumes(t *testing.T) {
	stableAfter(t, 200*time.Millisecond)
	var key any
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			key = start.m["idempotencyKey"]
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			time.Sleep(500 * time.Millisecond)
			c.send("invocationRejected", map[string]any{
				"attemptId": start.m["attemptId"], "code": "resource-exhausted", "message": "busy", "retryable": true,
			})
			c.waitClose()
		},
		func(c *fakeConn) {
			resume := c.expect("resumeAttach")
			if resume.m["operation"] != "resume" {
				t.Errorf("reattached with %v", resume.m)
			}
			acceptAs(c, resume, key, "sess2", outputMapping(6, "out"))
			outputItem(c, 6, 1, "c1", 20)
			outputEnd(c, 6, 2, "c2", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	out, err := CallStreaming(ctx, server.agentWithin(t, 300*time.Millisecond), "produce",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	values, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{10, 20}) {
		t.Fatalf("collected %v, %v", values, err)
	}
}

// An accepted connection that drops at once is no progress: the reconnects
// back off and the window keeps running from the open, so the call ends as
// unreachable.
func TestAnAcceptedConnectionDroppedAtOnceIsNoProgress(t *testing.T) {
	stableAfter(t, 200*time.Millisecond)
	var mu sync.Mutex
	var key any
	var opened []time.Time
	acceptAndDrop := func(c *fakeConn) {
		mu.Lock()
		opened = append(opened, time.Now())
		mu.Unlock()
		op := c.read()
		switch op.kind {
		case "invocationStart":
			mu.Lock()
			key = op.m["idempotencyKey"]
			mu.Unlock()
			accept(c, op, "sess")
		case "resumeAttach":
			mu.Lock()
			k := key
			mu.Unlock()
			acceptAs(c, op, k, "sess")
		default:
			t.Errorf("server got %s", op.kind)
		}
		c.drop()
	}
	server := newFakeServer(t, slices.Repeat([]func(*fakeConn){acceptAndDrop}, 100)...)
	started := time.Now()
	_, err := CallStreaming(ctxT(t), server.agentWithin(t, 600*time.Millisecond), "numbers",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	elapsed := time.Since(started)
	var unreachable *UnreachableError
	if !errors.As(err, &unreachable) {
		t.Fatalf("got %v", err)
	}
	if elapsed < 600*time.Millisecond || elapsed > 3*time.Second {
		t.Fatalf("gave up after %v", elapsed)
	}
	mu.Lock()
	defer mu.Unlock()
	// The reconnects wait 50, 100, 200 and 400 ms.
	if n := len(opened); n < 3 || n > 6 {
		t.Fatalf("connected %d times", n)
	}
	if gap := opened[len(opened)-1].Sub(opened[len(opened)-2]); gap < 150*time.Millisecond {
		t.Fatalf("the last reconnect waited only %v", gap)
	}
}

// Accepted connections that stay up keep a call alive past the window, and
// each loss reconnects at once.
func TestStableConnectionsKeepTheCallAlivePastTheWindow(t *testing.T) {
	stableAfter(t, 100*time.Millisecond)
	var mu sync.Mutex
	var key any
	var lost time.Time
	var waits []time.Duration
	reconnected := func() any {
		mu.Lock()
		defer mu.Unlock()
		waits = append(waits, time.Since(lost))
		return key
	}
	dropNow := func(c *fakeConn) {
		mu.Lock()
		lost = time.Now()
		mu.Unlock()
		c.drop()
	}
	holdAndDrop := func(c *fakeConn) {
		k := reconnected()
		resume := c.expect("resumeAttach")
		acceptAs(c, resume, k, "sess", outputMapping(2, "out"))
		time.Sleep(250 * time.Millisecond)
		dropNow(c)
	}
	server := newFakeServer(t,
		func(c *fakeConn) {
			start := c.expect("invocationStart")
			mu.Lock()
			key = start.m["idempotencyKey"]
			mu.Unlock()
			accept(c, start, "sess")
			streamResult(c, 2, "out", u32StreamGraph)
			outputItem(c, 2, 0, "c0", 10)
			time.Sleep(250 * time.Millisecond)
			dropNow(c)
		},
		holdAndDrop, holdAndDrop, holdAndDrop,
		func(c *fakeConn) {
			k := reconnected()
			resume := c.expect("resumeAttach")
			acceptAs(c, resume, k, "sess", outputMapping(2, "out"))
			outputItem(c, 2, 1, "c1", 20)
			outputEnd(c, 2, 2, "c2", map[string]any{"kind": "ok"})
			finished(c)
			c.waitClose()
		},
	)
	ctx := ctxT(t)
	started := time.Now()
	out, err := CallStreaming(ctx, server.agentWithin(t, 400*time.Millisecond), "produce",
		schema.RecordValue{}, DecodeStream(DecodeU32))
	if err != nil {
		t.Fatal(err)
	}
	values, err := out.Collect(ctx)
	if err != nil || !reflect.DeepEqual(values, []uint32{10, 20}) {
		t.Fatalf("collected %v, %v", values, err)
	}
	if elapsed := time.Since(started); elapsed < time.Second {
		t.Fatalf("the call ended after %v, within the window", elapsed)
	}
	mu.Lock()
	defer mu.Unlock()
	for _, wait := range waits {
		if wait > 40*time.Millisecond {
			t.Fatalf("a reconnect after a stable connection waited %v", wait)
		}
	}
}

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
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// An invocation session carries one call to a streaming method over a
// WebSocket: the call itself, every stream argument, the result and every
// stream the result holds.
//
// The connection is not the session. When it drops, the session reconnects and
// resumes: each input resends what the server had not yet durably accepted,
// and each output continues after the last item handed to the reader. Before
// the server accepted the call, the identical request is sent again, so a call
// is never started twice.

// SessionError is a call the server rejected or an invocation that failed,
// with the protocol's stable error code.
type SessionError struct {
	Code    string
	Message string
}

func (e *SessionError) Error() string {
	return fmt.Sprintf("golem: invocation session failed (%s): %s", e.Code, e.Message)
}

const (
	pingInterval    = time.Second
	pingTimeout     = 3 * time.Second
	firstRetryDelay = 50 * time.Millisecond
	maxRetryDelay   = 3 * time.Second
	// reconnectWindow bounds how long a session keeps trying to reach the
	// server before it gives up.
	reconnectWindow = 5 * time.Minute
)

type session struct {
	agent  *Agent
	method string
	idem   string

	// life ends when the session does; it bounds every dial and read.
	life    context.Context
	endLife context.CancelFunc

	mu   sync.Mutex
	conn *sessionConn

	params        schema.SchemaValue
	inputs        []*inputStream
	byProvisional map[string]*inputStream
	byToken       map[string]*inputStream
	outputs       map[string]*outputStream
	directions    map[string]string
	channels      map[uint32]string
	tokenChannels map[string]uint32
	cursors       map[string]string

	attempt      string
	descriptor   []byte
	accepted     bool
	everAccepted bool
	sessionToken string

	resultSent bool
	result     chan sessionResult
	over       bool
	err        error
	done       chan struct{}

	queued  int
	unacked int
}

type sessionResult struct {
	value schema.SchemaValue
	err   error
}

// openSession starts a call. The call is sent once the connection is up; the
// result arrives on s.result.
func openSession(a *Agent, method string, params schema.SchemaValue) (*session, error) {
	life, end := context.WithCancel(context.Background())
	s := &session{
		agent:         a,
		method:        method,
		idem:          newUUID(),
		life:          life,
		endLife:       end,
		byProvisional: map[string]*inputStream{},
		byToken:       map[string]*inputStream{},
		outputs:       map[string]*outputStream{},
		directions:    map[string]string{},
		channels:      map[uint32]string{},
		tokenChannels: map[string]uint32{},
		cursors:       map[string]string{},
		result:        make(chan sessionResult, 1),
		done:          make(chan struct{}),
	}
	seen := map[*inputHandle]bool{}
	var err error
	s.params, err = mapStreams(params, func(v schema.StreamValue) (schema.SchemaValue, error) {
		h, ok := v.Handle.(*inputHandle)
		if !ok {
			return nil, errors.New("golem: a stream argument must be a bridge.AgentStream")
		}
		if h.err != nil {
			return nil, h.err
		}
		if seen[h] {
			return nil, ErrStreamConsumed
		}
		seen[h] = true
		s.inputs = append(s.inputs, &inputStream{s: s, h: h, wake: make(chan struct{}, 1)})
		return v, nil
	})
	if err != nil {
		end()
		return nil, fmt.Errorf("golem: %s.%s arguments: %w", a.typeName, method, err)
	}
	if err := s.prepareStart(); err != nil {
		end()
		return nil, err
	}
	go s.connect(0)
	return s, nil
}

// prepareStart freezes a new start operation: a fresh attempt, and fresh
// provisional references for the stream arguments.
func (s *session) prepareStart() error {
	clear(s.byProvisional)
	byHandle := map[*inputHandle]*inputStream{}
	for _, in := range s.inputs {
		in.provisional = newUUID()
		s.byProvisional[in.provisional] = in
		byHandle[in.h] = in
	}
	params, err := mapStreams(s.params, func(v schema.StreamValue) (schema.SchemaValue, error) {
		in := byHandle[v.Handle.(*inputHandle)]
		return schema.StreamValue{Handle: schema.WireStreamRef{ProvisionalRef: in.provisional}}, nil
	})
	if err != nil {
		return err
	}
	a := s.agent
	constructor, err := schema.MarshalWireValue(a.parameters)
	if err != nil {
		return fmt.Errorf("golem: %s constructor arguments: %w", a.typeName, err)
	}
	methodParams, err := schema.MarshalWireValue(params)
	if err != nil {
		return fmt.Errorf("golem: %s.%s arguments: %w", a.typeName, s.method, err)
	}
	selector := map[string]any{
		"agentType":             a.typeName,
		"application":           a.configuration.AppName,
		"constructorParameters": json.RawMessage(constructor),
		"environment":           a.configuration.EnvName,
		"method":                s.method,
	}
	if a.phantomID != nil {
		selector["phantomId"] = *a.phantomID
	}
	s.attempt = newUUID()
	s.descriptor, err = encodeMessage("invocationStart", map[string]any{
		"attemptId":        s.attempt,
		"config":           configEntriesToDTO(a.config),
		"idempotencyKey":   s.idem,
		"methodParameters": json.RawMessage(methodParams),
		"selector":         selector,
	})
	return err
}

// prepareResume freezes a new resume operation. Called with s.mu held.
func (s *session) prepareResume() error {
	cursors := make([]string, 0, len(s.cursors))
	for _, c := range s.cursors {
		cursors = append(cursors, c)
	}
	slices.Sort(cursors)
	s.attempt = newUUID()
	var err error
	s.descriptor, err = encodeMessage("resumeAttach", map[string]any{
		"attemptId":     s.attempt,
		"operation":     "resume",
		"outputCursors": cursors,
		"sessionToken":  s.sessionToken,
	})
	s.accepted = false
	return err
}

// --- the connection ------------------------------------------------------

type outFrame struct {
	binary bool
	data   []byte
	close  bool
}

// sessionConn is one WebSocket. Frames are written in order by one goroutine;
// a frame queued for a connection that has since dropped is discarded, since
// whatever it carried is resent after the session resumes.
type sessionConn struct {
	ws     *websocket.Conn
	mu     sync.Mutex
	queue  []outFrame
	wake   chan struct{}
	closed chan struct{}
	once   sync.Once
}

func (c *sessionConn) enqueue(f outFrame) {
	c.mu.Lock()
	c.queue = append(c.queue, f)
	c.mu.Unlock()
	poke(c.wake)
}

func (c *sessionConn) shut() {
	c.once.Do(func() {
		close(c.closed)
		_ = c.ws.CloseNow()
	})
}

func poke(ch chan struct{}) {
	select {
	case ch <- struct{}{}:
	default:
	}
}

func (s *session) send(kind string, members map[string]any) {
	if s.conn == nil {
		return
	}
	data, err := encodeMessage(kind, members)
	if err != nil {
		go s.fail(err)
		return
	}
	s.conn.enqueue(outFrame{data: data})
}

func (s *session) sendCancel(channel uint32, reason string) {
	s.send("streamCancel", map[string]any{"channel": channel, "reason": reason})
}

func (s *session) url() string {
	base := s.agent.configuration.Server.URL()
	switch {
	case strings.HasPrefix(base, "https://"):
		base = "wss://" + strings.TrimPrefix(base, "https://")
	case strings.HasPrefix(base, "http://"):
		base = "ws://" + strings.TrimPrefix(base, "http://")
	}
	return base + sessionEndpoint
}

// connect dials until a connection is up, then sends the frozen operation.
func (s *session) connect(delay time.Duration) {
	started := time.Now()
	for {
		if delay > 0 {
			select {
			case <-time.After(delay):
			case <-s.life.Done():
				return
			}
		}
		delay = min(max(delay*2, firstRetryDelay), maxRetryDelay)
		options := &websocket.DialOptions{
			Subprotocols:    []string{sessionSubprotocol},
			HTTPHeader:      http.Header{"Authorization": {"Bearer " + s.agent.configuration.Server.Token()}},
			CompressionMode: websocket.CompressionDisabled,
		}
		if client := s.agent.configuration.client(); client.Timeout == 0 {
			options.HTTPClient = client
		}
		ws, response, err := websocket.Dial(s.life, s.url(), options)
		if err != nil {
			if s.life.Err() != nil {
				return
			}
			if response != nil && !retryableStatus(response.StatusCode) {
				s.fail(&Error{Endpoint: "invoke-agent-session", Status: response.StatusCode, Err: err})
				return
			}
			if time.Since(started) > reconnectWindow {
				s.fail(&Error{Endpoint: "invoke-agent-session", Err: err})
				return
			}
			continue
		}
		if ws.Subprotocol() != sessionSubprotocol {
			_ = ws.CloseNow()
			s.fail(protocolErrorf("the server did not select %s", sessionSubprotocol))
			return
		}
		ws.SetReadLimit(maxMessageBytes + 64)
		c := &sessionConn{ws: ws, wake: make(chan struct{}, 1), closed: make(chan struct{})}
		s.mu.Lock()
		if s.over {
			s.mu.Unlock()
			c.shut()
			return
		}
		s.conn = c
		c.queue = append(c.queue, outFrame{data: s.descriptor})
		s.mu.Unlock()
		go s.write(c)
		go s.read(c)
		go s.ping(c)
		return
	}
}

func retryableStatus(status int) bool {
	return status == http.StatusTooManyRequests || status >= 500
}

func (s *session) write(c *sessionConn) {
	for {
		c.mu.Lock()
		queue := c.queue
		c.queue = nil
		c.mu.Unlock()
		for _, f := range queue {
			if f.close {
				_ = c.ws.Close(websocket.StatusNormalClosure, "")
				c.shut()
				s.endLife()
				return
			}
			kind := websocket.MessageText
			if f.binary {
				kind = websocket.MessageBinary
			}
			if err := c.ws.Write(s.life, kind, f.data); err != nil {
				s.lost(c)
				return
			}
		}
		select {
		case <-c.wake:
		case <-c.closed:
			return
		}
	}
}

func (s *session) read(c *sessionConn) {
	for {
		kind, data, err := c.ws.Read(s.life)
		if err != nil {
			s.lost(c)
			return
		}
		if kind == websocket.MessageText {
			err = s.handleText(c, data)
		} else {
			err = s.handleBinary(c, data)
		}
		if err != nil {
			s.fail(err)
			return
		}
	}
}

func (s *session) ping(c *sessionConn) {
	ticker := time.NewTicker(pingInterval)
	defer ticker.Stop()
	for {
		select {
		case <-c.closed:
			return
		case <-ticker.C:
		}
		ctx, cancel := context.WithTimeout(s.life, pingTimeout)
		err := c.ws.Ping(ctx)
		cancel()
		if err != nil {
			s.lost(c)
			return
		}
	}
}

// lost handles a connection that dropped: the session detaches every stream
// and reconnects.
func (s *session) lost(c *sessionConn) {
	s.mu.Lock()
	if s.conn != c || s.over {
		s.mu.Unlock()
		c.shut()
		return
	}
	err := s.detach()
	s.mu.Unlock()
	c.shut()
	if err != nil {
		s.fail(err)
		return
	}
	go s.connect(0)
}

// detach forgets the connection-local channel table and freezes the next
// operation: the same start again before acceptance, a resume after it.
// Called with s.mu held.
func (s *session) detach() error {
	s.conn = nil
	clear(s.channels)
	clear(s.tokenChannels)
	for _, in := range s.byToken {
		in.channel = 0
	}
	for _, out := range s.outputs {
		out.resume()
	}
	if s.accepted {
		return s.prepareResume()
	}
	return nil
}

// fail ends the session with an error every open stream and the caller see.
func (s *session) fail(err error) {
	s.mu.Lock()
	if s.over {
		s.mu.Unlock()
		return
	}
	s.over = true
	s.err = err
	if !s.resultSent {
		s.resultSent = true
		s.result <- sessionResult{err: err}
	}
	for _, out := range s.outputs {
		if !out.ended {
			out.failure = err
			poke(out.wake)
		}
	}
	inputs := slices.Collect(func(yield func(*inputStream) bool) {
		for _, in := range s.inputs {
			if !yield(in) {
				return
			}
		}
		for _, in := range s.byProvisional {
			if !yield(in) {
				return
			}
		}
	})
	c := s.conn
	s.conn = nil
	close(s.done)
	s.mu.Unlock()
	for _, in := range inputs {
		in.h.stop("cancelled")
	}
	if c != nil {
		c.enqueue(outFrame{close: true})
	} else {
		s.endLife()
	}
}

// abort ends a call its caller gave up on: open streams are cancelled first.
func (s *session) abort(err error) {
	s.mu.Lock()
	if !s.over {
		for token, ch := range s.tokenChannels {
			switch s.directions[token] {
			case "input":
				if in := s.byToken[token]; in != nil && !in.ended {
					s.sendCancel(ch, "cancelled")
				}
			case "output":
				if out := s.outputs[token]; out != nil && !out.terminal {
					s.sendCancel(ch, "cancelled")
				}
			}
		}
	}
	s.mu.Unlock()
	s.fail(err)
}

// finish ends a session the server completed.
func (s *session) finish() {
	s.mu.Lock()
	if s.over {
		s.mu.Unlock()
		return
	}
	s.over = true
	c := s.conn
	s.conn = nil
	close(s.done)
	s.mu.Unlock()
	if c != nil {
		c.enqueue(outFrame{close: true})
	} else {
		s.endLife()
	}
}

// await waits for the result. A caller that gives up first cancels the call.
func (s *session) await(ctx context.Context) sessionResult {
	select {
	case r := <-s.result:
		return r
	case <-ctx.Done():
		s.abort(ctx.Err())
		return sessionResult{err: ctx.Err()}
	}
}

// --- incoming messages -------------------------------------------------------

func (s *session) handleText(c *sessionConn, data []byte) error {
	kind, m, err := parseMessage(data)
	if err != nil {
		return err
	}
	switch kind {
	case "invocationAccepted":
		if err := m.members(kind, []string{"attemptId", "idempotencyKey", "mappings", "sessionToken"}); err != nil {
			return err
		}
		return s.accept(c, m)
	case "invocationRejected":
		if err := m.members(kind, []string{"code", "message", "retryable"}, "attemptId"); err != nil {
			return err
		}
		return s.rejected(c, m)
	}

	s.mu.Lock()
	accepted := s.accepted && s.conn == c
	s.mu.Unlock()
	if !accepted {
		return protocolErrorf("%s arrived before the invocation was accepted", kind)
	}
	switch kind {
	case "invocationResult":
		if err := m.members(kind, []string{"mappings", "result"}); err != nil {
			return err
		}
		return s.receiveResult(c, m)
	case "outputStreamItem":
		if err := m.members(kind, []string{"channel", "cursorToken", "mappings", "sequence", "value"}); err != nil {
			return err
		}
		return s.receiveItem(c, m)
	case "outputStreamEnd":
		if err := m.members(kind, []string{"channel", "outcome", "sequence"}, "cursorToken"); err != nil {
			return err
		}
		return s.receiveEnd(c, m)
	case "inputStreamAck":
		if err := m.members(kind, []string{"channel", "highestContiguousSequence", "mappings", "terminal"}); err != nil {
			return err
		}
		return s.receiveAck(c, m)
	case "streamCancel":
		if err := m.members(kind, []string{"channel", "reason"}); err != nil {
			return err
		}
		return s.receiveCancel(c, m)
	case "attachmentRevoked":
		if err := m.members(kind, []string{"reason"}); err != nil {
			return err
		}
		if reason, err := m.str("reason"); err != nil || reason != "replaced" {
			return protocolErrorf("invalid revocation reason")
		}
		s.lost(c)
		return nil
	case "invocationFinished":
		if err := m.members(kind, []string{"outcome"}); err != nil {
			return err
		}
		return s.receiveFinished(m)
	}
	return protocolErrorf("unknown message type %q", kind)
}

func (s *session) accept(c *sessionConn, m message) error {
	attempt, err := m.str("attemptId")
	if err != nil {
		return err
	}
	key, err := m.str("idempotencyKey")
	if err != nil {
		return err
	}
	token, err := m.token("sessionToken")
	if err != nil {
		return err
	}
	mappings, err := m.mappings()
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.conn != c {
		return nil
	}
	if s.accepted {
		return protocolErrorf("duplicate invocation acceptance")
	}
	if attempt != s.attempt || key != s.idem {
		return protocolErrorf("the acceptance does not match the pending operation")
	}
	if err := s.install(mappings, true); err != nil {
		return err
	}
	s.sessionToken = token
	s.accepted = true
	s.everAccepted = true
	return nil
}

func (s *session) rejected(c *sessionConn, m message) error {
	code, err := m.str("code")
	if err != nil {
		return err
	}
	text, err := m.str("message")
	if err != nil {
		return err
	}
	retryable, err := m.boolean("retryable")
	if err != nil {
		return err
	}
	s.mu.Lock()
	if s.conn != c {
		s.mu.Unlock()
		return nil
	}
	if m.has("attemptId") {
		attempt, err := m.str("attemptId")
		if err != nil || attempt != s.attempt {
			s.mu.Unlock()
			return protocolErrorf("the rejection does not match the pending operation")
		}
	} else if s.everAccepted {
		s.mu.Unlock()
		return protocolErrorf("a rejection after acceptance must name its attempt")
	}
	if !retryable {
		s.mu.Unlock()
		return &SessionError{Code: code, Message: text}
	}
	// A retryable rejection means a new attempt may reach the agent: a resume
	// once the call was accepted, otherwise a fresh start.
	if err := s.detach(); err != nil {
		s.mu.Unlock()
		return err
	}
	if !s.everAccepted {
		if err := s.prepareStart(); err != nil {
			s.mu.Unlock()
			return err
		}
	}
	s.mu.Unlock()
	c.shut()
	go s.connect(firstRetryDelay)
	return nil
}

// install applies stream mappings. Called with s.mu held.
func (s *session) install(mappings []mapping, complete bool) error {
	for _, mp := range mappings {
		if dir, ok := s.directions[mp.token]; ok && dir != mp.direction {
			return protocolErrorf("a stream token was rebound to the other direction")
		}
		if token, ok := s.channels[mp.channel]; ok && token != mp.token {
			return protocolErrorf("a stream channel was rebound")
		}
		if ch, ok := s.tokenChannels[mp.token]; ok && ch != mp.channel {
			return protocolErrorf("a stream token received two channels")
		}
		if _, known := s.directions[mp.token]; !known && len(s.directions) >= maxMappings {
			return protocolErrorf("too many stream mappings")
		}
		switch mp.direction {
		case "input":
			in := s.byToken[mp.token]
			if mp.provisional != "" {
				byRef, ok := s.byProvisional[mp.provisional]
				if !ok {
					return protocolErrorf("an input mapping names an unknown provisional reference")
				}
				if in != nil && in != byRef {
					return protocolErrorf("an input stream token was rebound")
				}
				if byRef.token != "" && byRef.token != mp.token {
					return protocolErrorf("an input stream was rebound to another token")
				}
				in = byRef
			}
			if in == nil {
				return protocolErrorf("an input mapping does not name a stream this call sent")
			}
		case "output":
			if mp.provisional != "" {
				return protocolErrorf("a provisional reference was mapped as an output")
			}
		}
	}
	if complete {
		if s.everAccepted {
			for token := range s.directions {
				if !slices.ContainsFunc(mappings, func(mp mapping) bool { return mp.token == token }) {
					return protocolErrorf("the resumed acceptance omitted a known stream")
				}
			}
		} else {
			for ref := range s.byProvisional {
				if !slices.ContainsFunc(mappings, func(mp mapping) bool { return mp.provisional == ref }) {
					return protocolErrorf("the acceptance omitted a stream argument")
				}
			}
		}
	}
	for _, mp := range mappings {
		s.directions[mp.token] = mp.direction
		s.channels[mp.channel] = mp.token
		s.tokenChannels[mp.token] = mp.channel
		switch mp.direction {
		case "input":
			in := s.byToken[mp.token]
			if in == nil {
				in = s.byProvisional[mp.provisional]
				in.token = mp.token
				s.byToken[mp.token] = in
			}
			if err := in.remap(mp.channel, mp.highWater, mp.terminal); err != nil {
				return err
			}
		case "output":
			out := s.outputs[mp.token]
			if out == nil {
				out = newOutputStream(s, mp.token)
				s.outputs[mp.token] = out
			}
			out.channel = mp.channel
			if out.pendingCancel != "" && !out.terminal {
				s.sendCancel(mp.channel, out.pendingCancel)
				out.pendingCancel = ""
			}
		}
	}
	return nil
}

func (s *session) installFrom(c *sessionConn, m message) (bool, error) {
	mappings, err := m.mappings()
	if err != nil {
		return false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.conn != c {
		return false, nil
	}
	return true, s.install(mappings, false)
}

func (s *session) receiveResult(c *sessionConn, m message) error {
	current, err := s.installFrom(c, m)
	if err != nil || !current {
		return err
	}
	s.mu.Lock()
	replayed := s.resultSent
	s.mu.Unlock()
	if replayed {
		// A resumed session may hear the result again; the caller already has it.
		return nil
	}
	result, err := m.object("result")
	if err != nil {
		return err
	}
	kind, err := result.str("kind")
	if err != nil {
		return err
	}
	var value schema.SchemaValue
	switch kind {
	case "none":
		if err := result.membersOnly("invocation result", []string{"kind"}); err != nil {
			return err
		}
	case "value":
		if err := result.membersOnly("invocation result", []string{"graph", "kind", "value"}); err != nil {
			return err
		}
		graph, err := schema.UnmarshalWireGraph(result["graph"])
		if err != nil {
			return protocolErrorf("result graph: %v", err)
		}
		raw, err := schema.UnmarshalWireValue(result["value"])
		if err != nil {
			return protocolErrorf("result value: %v", err)
		}
		ref := schema.NewRef(graph)
		if err := ref.Validate(raw); err != nil {
			return protocolErrorf("result value: %v", err)
		}
		s.mu.Lock()
		value, err = s.bindOutputs(ref, ref.Type(), raw)
		s.mu.Unlock()
		if err != nil {
			return err
		}
	default:
		return protocolErrorf("invalid invocation result kind %q", kind)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.resultSent {
		return nil
	}
	s.resultSent = true
	s.result <- sessionResult{value: value}
	return nil
}

// bindOutputs replaces each stream token in a received value with the session
// stream it names, remembering its item type. Called with s.mu held.
func (s *session) bindOutputs(ref schema.Ref, t schema.SchemaType, v schema.SchemaValue) (schema.SchemaValue, error) {
	return mapTypedStreams(ref, t, v, func(item *schema.SchemaType, sv schema.StreamValue) (schema.SchemaValue, error) {
		wire, ok := sv.Handle.(schema.WireStreamRef)
		if !ok || wire.StreamToken == "" {
			return nil, protocolErrorf("the server returned a stream without a stable token")
		}
		if err := checkToken(wire.StreamToken); err != nil {
			return nil, protocolErrorf("stream token: %v", err)
		}
		out := s.outputs[wire.StreamToken]
		if out == nil || s.directions[wire.StreamToken] != "output" {
			return nil, protocolErrorf("a returned stream was not announced as an output")
		}
		if out.bound {
			return nil, protocolErrorf("a stream appears twice in what the server returned")
		}
		if item == nil {
			return nil, protocolErrorf("a returned stream has no item type")
		}
		out.bound = true
		out.item = ref.At(*item)
		out.lane = laneOfType(out.item)
		return schema.StreamValue{Handle: out}, nil
	})
}

func laneOfType(r schema.Ref) lane {
	resolved, err := r.Resolved()
	if err != nil {
		return laneJSON
	}
	switch resolved.Type().Body.(type) {
	case schema.U8Type:
		return laneU8
	case schema.BinaryType:
		return laneBinary
	}
	return laneJSON
}

func (s *session) outputOn(channel uint32) (*outputStream, error) {
	token, ok := s.channels[channel]
	if !ok || s.directions[token] != "output" {
		return nil, protocolErrorf("channel %d is not an output", channel)
	}
	out := s.outputs[token]
	if !out.bound {
		return nil, protocolErrorf("an item arrived for a stream that was never returned")
	}
	return out, nil
}

func (s *session) receiveItem(c *sessionConn, m message) error {
	current, err := s.installFrom(c, m)
	if err != nil || !current {
		return err
	}
	channel, err := m.channel()
	if err != nil {
		return err
	}
	sequence, err := m.u64("sequence")
	if err != nil {
		return err
	}
	cursor, err := m.token("cursorToken")
	if err != nil {
		return err
	}
	raw, err := schema.UnmarshalWireValue(m["value"])
	if err != nil {
		return protocolErrorf("stream item: %v", err)
	}
	s.mu.Lock()
	out, err := s.outputOn(channel)
	if err != nil {
		s.mu.Unlock()
		return err
	}
	if out.lane != laneJSON {
		s.mu.Unlock()
		return protocolErrorf("a %s stream's item arrived as JSON", out.lane)
	}
	if err := out.item.Validate(raw); err != nil {
		s.mu.Unlock()
		return protocolErrorf("stream item: %v", err)
	}
	value, err := s.bindOutputs(out.item, out.item.Type(), raw)
	s.mu.Unlock()
	if err != nil {
		return err
	}
	return out.offer(c, outEntry{
		first: sequence, count: 1, value: value, cursor: cursor, bytes: len(m["value"]),
	})
}

func (s *session) handleBinary(c *sessionConn, data []byte) error {
	f, err := decodeBinary(data)
	if err != nil {
		return err
	}
	s.mu.Lock()
	if s.conn != c {
		s.mu.Unlock()
		return nil
	}
	if !s.accepted {
		s.mu.Unlock()
		return protocolErrorf("a binary frame arrived before the invocation was accepted")
	}
	out, err := s.outputOn(f.channel)
	s.mu.Unlock()
	if err != nil {
		return err
	}
	entry := outEntry{first: f.sequence, count: f.count, cursor: f.cursor, bytes: len(f.payload)}
	switch f.kind {
	case "output-u8":
		if out.lane != laneU8 {
			return protocolErrorf("packed bytes arrived for a %s stream", out.lane)
		}
		for _, b := range f.payload {
			if err := out.item.Validate(schema.U8Value{Value: b}); err != nil {
				return protocolErrorf("stream item: %v", err)
			}
		}
		entry.packed = f.payload
	case "output-binary":
		if out.lane != laneBinary {
			return protocolErrorf("a binary item arrived for a %s stream", out.lane)
		}
		value := schema.BinaryValue{Bytes: f.payload, MimeType: f.mime}
		if err := out.item.Validate(value); err != nil {
			return protocolErrorf("stream item: %v", err)
		}
		entry.value = value
	default:
		return protocolErrorf("the server sent a %s frame", f.kind)
	}
	return out.offer(c, entry)
}

func (s *session) receiveEnd(c *sessionConn, m message) error {
	channel, err := m.channel()
	if err != nil {
		return err
	}
	sequence, err := m.u64("sequence")
	if err != nil {
		return err
	}
	var cursor string
	if m.has("cursorToken") {
		if cursor, err = m.token("cursorToken"); err != nil {
			return err
		}
	}
	outcome, err := m.object("outcome")
	if err != nil {
		return err
	}
	kind, err := outcome.str("kind")
	if err != nil {
		return err
	}
	var end error
	switch kind {
	case "ok":
		err = outcome.membersOnly("stream outcome", []string{"kind"})
	case "error":
		if err = outcome.membersOnly("stream outcome", []string{"code", "kind", "message"}); err == nil {
			failure := &StreamError{}
			if failure.Code, err = outcome.str("code"); err == nil {
				failure.Message, err = outcome.str("message")
			}
			end = failure
		}
	case "cancelled":
		if err = outcome.membersOnly("stream outcome", []string{"kind", "reason"}); err == nil {
			var reason string
			if reason, err = outcome.str("reason"); err == nil && !slices.Contains(cancelReasons, reason) {
				err = protocolErrorf("invalid cancellation reason %q", reason)
			}
			end = &StreamCancelled{Reason: reason}
		}
	default:
		err = protocolErrorf("invalid stream outcome %q", kind)
	}
	if err != nil {
		return err
	}
	s.mu.Lock()
	if s.conn != c {
		s.mu.Unlock()
		return nil
	}
	out, err := s.outputOn(channel)
	s.mu.Unlock()
	if err != nil {
		return err
	}
	return out.offer(c, outEntry{first: sequence, cursor: cursor, terminal: true, end: end})
}

func (s *session) receiveAck(c *sessionConn, m message) error {
	current, err := s.installFrom(c, m)
	if err != nil || !current {
		return err
	}
	channel, err := m.channel()
	if err != nil {
		return err
	}
	high, err := m.u64("highestContiguousSequence")
	if err != nil {
		return err
	}
	terminal, err := m.boolean("terminal")
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	token, ok := s.channels[channel]
	if !ok || s.directions[token] != "input" {
		return protocolErrorf("an acknowledgement arrived on channel %d, which is not an input", channel)
	}
	return s.byToken[token].acknowledge(high, terminal)
}

func (s *session) receiveCancel(c *sessionConn, m message) error {
	channel, err := m.channel()
	if err != nil {
		return err
	}
	reason, err := m.str("reason")
	if err != nil {
		return err
	}
	if !slices.Contains(cancelReasons, reason) {
		return protocolErrorf("invalid cancellation reason %q", reason)
	}
	s.mu.Lock()
	token, ok := s.channels[channel]
	if s.conn != c {
		s.mu.Unlock()
		return nil
	}
	if !ok || s.directions[token] != "input" {
		s.mu.Unlock()
		return protocolErrorf("a cancellation arrived on channel %d, which is not an input", channel)
	}
	in := s.byToken[token]
	in.cancelled = true
	in.ended = true
	in.release()
	poke(in.wake)
	s.mu.Unlock()
	in.h.stop(reason)
	return nil
}

func (s *session) receiveFinished(m message) error {
	outcome, err := m.object("outcome")
	if err != nil {
		return err
	}
	kind, err := outcome.str("kind")
	if err != nil {
		return err
	}
	switch kind {
	case "success":
		if err := outcome.membersOnly("invocation outcome", []string{"kind"}); err != nil {
			return err
		}
		s.mu.Lock()
		resultSent := s.resultSent
		open := slices.ContainsFunc(slices.Collect(mapValues(s.outputs)), func(o *outputStream) bool { return !o.terminal })
		s.mu.Unlock()
		if !resultSent {
			return protocolErrorf("the invocation finished before its result")
		}
		if open {
			return protocolErrorf("the invocation finished before a stream it returned ended")
		}
		s.finish()
		return nil
	case "failure":
		if err := outcome.membersOnly("invocation outcome", []string{"code", "kind", "message"}); err != nil {
			return err
		}
		code, err := outcome.str("code")
		if err != nil {
			return err
		}
		text, err := outcome.str("message")
		if err != nil {
			return err
		}
		s.fail(&SessionError{Code: code, Message: text})
		return nil
	}
	return protocolErrorf("invalid invocation outcome %q", kind)
}

func mapValues[K comparable, V any](m map[K]V) func(func(V) bool) {
	return func(yield func(V) bool) {
		for _, v := range m {
			if !yield(v) {
				return
			}
		}
	}
}

func newUUID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(fmt.Sprintf("golem: no randomness for a UUID: %v", err))
	}
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

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

// Package websocket is a Go wrapper over Golem's durable WebSocket client
// (golem:websocket). Connect to a URL, then Send/Receive text or binary frames.
//
// Like the other WASI wrappers, operations return an error (a receive timeout is
// reported as ok=false, not an error), distinct from the fail-loud control-flow
// surface. The connection is durable — sends/receives are journaled and replayed
// — and because they are remote side effects, using it inside a read-only method
// traps.
//
// Pair a fallible call with golem.Must / golem.Must0 / golem.Must2 to abort the
// invocation on error.
package websocket

import (
	"fmt"
	"time"

	client "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_websocket_client"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// ── Messages ──────────────────────────────────────────────────────────────────

// Message is a WebSocket frame — either text or binary. Build one with
// [TextMessage] or [BinaryMessage].
type Message struct {
	binary bool
	text   string
	data   []byte
}

// TextMessage builds a text frame.
func TextMessage(s string) Message { return Message{text: s} }

// BinaryMessage builds a binary frame.
func BinaryMessage(b []byte) Message { return Message{binary: true, data: b} }

// IsText reports whether the message is a text frame.
func (m Message) IsText() bool { return !m.binary }

// IsBinary reports whether the message is a binary frame.
func (m Message) IsBinary() bool { return m.binary }

// Text returns the text payload (empty for a binary frame).
func (m Message) Text() string { return m.text }

// Bytes returns the binary payload (nil for a text frame).
func (m Message) Bytes() []byte { return m.data }

func (m Message) toWit() client.Message {
	if m.binary {
		return client.MakeMessageBinary(m.data)
	}
	return client.MakeMessageText(m.text)
}

func messageFromWit(m client.Message) Message {
	if m.Tag() == client.MessageBinary {
		return Message{binary: true, data: m.Binary()}
	}
	return Message{text: m.Text()}
}

// ── Errors ────────────────────────────────────────────────────────────────────

// ErrorKind classifies a [Error].
type ErrorKind uint8

const (
	ConnectionFailure ErrorKind = iota
	SendFailure
	ReceiveFailure
	ProtocolError
	// Closed means the peer closed the connection; CloseCode/CloseReason may carry
	// the close frame.
	Closed
	Other
)

// Error is a WebSocket host error.
type Error struct {
	Kind        ErrorKind
	Message     string
	CloseCode   uint16
	CloseReason string
}

func (e *Error) Error() string { return "golem/websocket: " + e.Message }

// IsClosed reports whether the error is a peer close.
func (e *Error) IsClosed() bool { return e.Kind == Closed }

func wsError(e client.Error) error {
	switch e.Tag() {
	case client.ErrorConnectionFailure:
		return &Error{Kind: ConnectionFailure, Message: e.ConnectionFailure()}
	case client.ErrorSendFailure:
		return &Error{Kind: SendFailure, Message: e.SendFailure()}
	case client.ErrorReceiveFailure:
		return &Error{Kind: ReceiveFailure, Message: e.ReceiveFailure()}
	case client.ErrorProtocolError:
		return &Error{Kind: ProtocolError, Message: e.ProtocolError()}
	case client.ErrorClosed:
		er := &Error{Kind: Closed, Message: "connection closed"}
		if ci := e.Closed(); ci.IsSome() {
			info := ci.Some()
			er.CloseCode = info.Code
			er.CloseReason = info.Reason
			er.Message = fmt.Sprintf("connection closed: code %d %q", info.Code, info.Reason)
		}
		return er
	default:
		return &Error{Kind: Other, Message: e.Other()}
	}
}

// ── Connection ────────────────────────────────────────────────────────────────

// ConnectOpt configures a [Connect] call.
type ConnectOpt func(*connectOpts)

type connectOpts struct {
	headers []witTypes.Tuple2[string, string]
}

// WithHeader adds a request header to the connection handshake.
func WithHeader(key, value string) ConnectOpt {
	return func(o *connectOpts) {
		o.headers = append(o.headers, witTypes.Tuple2[string, string]{F0: key, F1: value})
	}
}

// Conn is an open WebSocket connection.
type Conn struct{ raw *client.WebsocketConnection }

// Connect opens a WebSocket connection to url.
func Connect(url string, opts ...ConnectOpt) (*Conn, error) {
	var o connectOpts
	for _, f := range opts {
		f(&o)
	}
	headers := witTypes.None[[]witTypes.Tuple2[string, string]]()
	if len(o.headers) > 0 {
		headers = witTypes.Some(o.headers)
	}
	r := client.WebsocketConnectionConnect(url, headers)
	if r.IsErr() {
		return nil, wsError(r.Err())
	}
	return &Conn{raw: r.Ok()}, nil
}

// Send sends a frame.
func (c *Conn) Send(m Message) error {
	if r := c.raw.Send(m.toWit()); r.IsErr() {
		return wsError(r.Err())
	}
	return nil
}

// SendText sends a text frame.
func (c *Conn) SendText(s string) error { return c.Send(TextMessage(s)) }

// SendBinary sends a binary frame.
func (c *Conn) SendBinary(b []byte) error { return c.Send(BinaryMessage(b)) }

// Receive blocks until a frame arrives (durably suspending the invocation).
func (c *Conn) Receive() (Message, error) {
	r := c.raw.Receive()
	if r.IsErr() {
		return Message{}, wsError(r.Err())
	}
	return messageFromWit(r.Ok()), nil
}

// ReceiveWithTimeout waits up to d for a frame. ok is false (nil error) if the
// wait times out.
func (c *Conn) ReceiveWithTimeout(d time.Duration) (msg Message, ok bool, err error) {
	r := c.raw.ReceiveWithTimeout(uint64(d.Milliseconds()))
	if r.IsErr() {
		return Message{}, false, wsError(r.Err())
	}
	opt := r.Ok()
	if opt.IsNone() {
		return Message{}, false, nil
	}
	return messageFromWit(opt.Some()), true, nil
}

// Close closes the connection with the given code and reason.
func (c *Conn) Close(code uint16, reason string) error {
	if r := c.raw.Close(witTypes.Some(code), witTypes.Some(reason)); r.IsErr() {
		return wsError(r.Err())
	}
	return nil
}

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
	"errors"
	"fmt"
	"time"
)

// Error is a call that did not produce a result. Endpoint always says which
// call it was; Status and Body are set only when the server answered, so a
// caller can tell a rejected request from one that never arrived.
type Error struct {
	Endpoint string
	Status   int
	Body     string
	Err      error
}

func (e *Error) Error() string {
	switch {
	case e.Status != 0 && e.Body != "":
		return fmt.Sprintf("golem: %s failed with HTTP %d: %s", e.Endpoint, e.Status, e.Body)
	case e.Status != 0 && e.Err != nil:
		return fmt.Sprintf("golem: %s failed with HTTP %d: %v", e.Endpoint, e.Status, e.Err)
	case e.Status != 0:
		return fmt.Sprintf("golem: %s failed with HTTP %d", e.Endpoint, e.Status)
	default:
		return fmt.Sprintf("golem: %s failed: %v", e.Endpoint, e.Err)
	}
}

func (e *Error) Unwrap() error { return e.Err }

// StreamItemError is an item of a stream argument that the call cannot send:
// one of the wrong kind for its stream's lane, one over the protocol's size
// limit, or one holding a stream that cannot be given away. It fails the call;
// Err says what was wrong with the item.
type StreamItemError struct{ Err error }

func (e *StreamItemError) Error() string { return e.Err.Error() }

func (e *StreamItemError) Unwrap() error { return e.Err }

// UnreachableError is a streaming call that could not reach the server within
// its reconnect window: the server accepted neither the start nor a resume. Endpoint
// is the WebSocket URL; Cause is the last failure, such as an *Error with the
// HTTP status of a refused upgrade, a *SessionError with a retryable
// rejection, a connection error, or ErrNotAnswered.
type UnreachableError struct {
	Endpoint string
	Window   time.Duration
	Cause    error
}

func (e *UnreachableError) Error() string {
	return fmt.Sprintf("golem: %s unreachable for %v: %v", e.Endpoint, e.Window, e.Cause)
}

func (e *UnreachableError) Unwrap() error { return e.Cause }

// ErrNotAnswered is the cause of an *UnreachableError when the connection was
// up but the server answered neither the start nor the resume within the
// reconnect window.
var ErrNotAnswered = errors.New("golem: the server answered neither the start nor the resume")

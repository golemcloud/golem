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

// Package observer owns a result resource independently of invocation cancellation.
package observer

import (
	"errors"
	"sync"
)

var ErrDetached = errors.New("golem: result observer is detached")

// Observer serializes observation and release without locking across a host Get.
// Decode transfers capabilities to the caller; Dispose releases an unclaimed raw result.
// CancelRaw and Drop must be synchronous and must not re-enter the observer;
// they run under its mutex so cancellation cannot address a released resource.
type Observer[R, O any] struct {
	mu                         sync.Mutex
	GetRaw                     func() R
	Decode                     func(R) (O, error)
	Dispose                    func(R)
	Drop                       func()
	CancelRaw                  func()
	started, detached, dropped bool
	done                       chan struct{}
	detachSignal               chan struct{}
	out                        O
	err                        error
	panicked                   any
}

func (o *Observer[R, O]) dropLocked() {
	if !o.dropped {
		o.dropped = true
		if o.Drop != nil {
			o.Drop()
		}
	}
}

func (o *Observer[R, O]) Get() (O, error) {
	o.mu.Lock()
	if o.detached {
		o.mu.Unlock()
		var zero O
		return zero, ErrDetached
	}
	if o.started {
		done := o.done
		o.mu.Unlock()
		<-done
		o.mu.Lock()
		defer o.mu.Unlock()
		if o.panicked != nil {
			panic(o.panicked)
		}
		return o.out, o.err
	}
	o.started = true
	o.done = make(chan struct{})
	o.mu.Unlock()
	// Always wake other observers, including when the host or decoder panics.
	defer func() {
		r := recover()
		o.mu.Lock()
		defer o.mu.Unlock()
		func() {
			defer func() {
				if dropPanic := recover(); r == nil {
					r = dropPanic
				}
			}()
			o.dropLocked()
		}()
		o.panicked = r
		close(o.done)
		if r != nil {
			panic(r)
		}
	}()
	raw := o.GetRaw()
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.detached {
		if o.Dispose != nil {
			o.Dispose(raw)
		}
		return o.out, ErrDetached
	}
	o.out, o.err = o.Decode(raw)
	return o.out, o.err
}

// Detach stops delivery, not execution. An active Get retains the physical
// resource until it settles. Already delivered capabilities belong to the caller.
func (o *Observer[R, O]) Detach() {
	o.mu.Lock()
	defer o.mu.Unlock()
	if !o.detached {
		if o.detachSignal == nil {
			o.detachSignal = make(chan struct{})
		}
		close(o.detachSignal)
	}
	o.detached = true
	var zero O
	o.out, o.err = zero, ErrDetached
	if !o.started || o.done == nil {
		o.dropLocked()
		return
	}
	select {
	case <-o.done:
		o.dropLocked()
	default:
	}
}

// Detached lets result collection stop waiting on independent output relays.
func (o *Observer[R, O]) Detached() <-chan struct{} {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.detachSignal == nil {
		o.detachSignal = make(chan struct{})
	}
	return o.detachSignal
}

// Cancel requests invocation cancellation, independently of detachment.
func (o *Observer[R, O]) Cancel() {
	o.mu.Lock()
	defer o.mu.Unlock()
	if !o.dropped && o.CancelRaw != nil {
		o.CancelRaw()
	}
}

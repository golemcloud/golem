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

//go:build wasip1

// Package cdctx gives every goroutine inside a custom durable operation its own
// host custom invocation context.
//
// The host keeps one context slot per component-model task, and every
// goroutine of an invocation shares that task. The Go runtime calls a switch
// hook whenever the goroutine about to run has a different context than the
// one installed, and the hook exchanges the host slot. Each party that is not
// installed holds the snapshot of its context: a goroutine in its Context, and
// every goroutine outside an operation (the root) in rootSaved.
package cdctx

import (
	"unsafe"

	witAsync "go.bytecodealliance.org/pkg/wit/async"
)

//go:wasmimport golem:durability/durability@1.6.0 exchange-custom-invocation-context
func exchangeImport(isSome int32, handle int32) int32

//go:wasmimport golem:durability/durability@1.6.0 [resource-drop]custom-invocation-context
func dropImport(handle int32)

//go:linkname setHooks runtime.wasiSetContextHooks
func setHooks(switchFn func(from, to unsafe.Pointer), create func(parent unsafe.Pointer) unsafe.Pointer, exit func(ctx unsafe.Pointer))

//go:linkname getContext runtime.wasiGetContext
func getContext() unsafe.Pointer

//go:linkname setContext runtime.wasiSetContext
func setContext(ctx unsafe.Pointer)

//go:linkname installContext runtime.wasiInstallContext
func installContext(ctx unsafe.Pointer)

//go:linkname domainEnter runtime.wasiDomainEnter
func domainEnter(ctx unsafe.Pointer) unsafe.Pointer

//go:linkname domainSetContext runtime.wasiDomainSetContext
func domainSetContext(dom, ctx unsafe.Pointer)

//go:linkname domainStop runtime.wasiDomainStop
func domainStop(dom unsafe.Pointer)

//go:linkname domainExit runtime.wasiDomainExit
func domainExit(dom unsafe.Pointer)

const none = int32(-1)

// Context is the host context of one goroutine inside a custom durable operation.
type Context struct {
	saved int32
	dead  bool
	op    *Operation
}

// Operation counts the goroutines started inside one custom durable operation
// that have not exited yet.
type Operation struct {
	live int32
	done chan struct{}
}

var rootSaved = none

func init() {
	setHooks(switchContext, created, exited)
}

func slot(c *Context) *int32 {
	if c == nil || c.dead {
		return &rootSaved
	}
	return &c.saved
}

// switchContext runs on the system stack: no allocation, no blocking.
func switchContext(from, to unsafe.Pointer) {
	fromSlot, toSlot := slot((*Context)(from)), slot((*Context)(to))
	if fromSlot == toSlot {
		return
	}
	next := *toSlot
	if next == none {
		panic("golem: custom invocation context has no saved snapshot")
	}
	*toSlot = none
	*fromSlot = exchangeImport(1, next)
}

// created runs on the parent goroutine at a go statement: the child starts in
// the parent's context and is counted until it exits.
func created(parent unsafe.Pointer) unsafe.Pointer {
	p := (*Context)(parent)
	if p.dead {
		return nil
	}
	c := &Context{saved: exchangeImport(0, 0), op: p.op}
	if c.op != nil {
		c.op.live++
	}
	return unsafe.Pointer(c)
}

// exited runs on an exiting goroutine after the root context is installed.
func exited(ctx unsafe.Pointer) {
	c := (*Context)(ctx)
	if c.dead {
		return
	}
	release(c)
	if op := c.op; op != nil {
		op.live--
		if op.live == 0 && op.done != nil {
			close(op.done)
		}
	}
}

func release(c *Context) {
	if c.saved != none {
		dropImport(c.saved)
		c.saved = none
	}
	c.dead = true
}

// Scope is one custom durable operation opened by Begin.
//
// The operation runs in its own runtime domain: its own clock, timers and
// random state. On replay a completed body is skipped, so state that the body
// shares with the rest of the program would carry the body's effects out of it.
// Begin draws the new domain's seed from the caller's random state on both
// paths, so the caller's stream advances by the same amount whether or not the
// body runs.
type Scope struct {
	c     *Context
	outer *Operation
	dom   unsafe.Pointer
	// timers is the context that the domain's host clock waits, and the
	// goroutines its timer functions start, run in: a snapshot of the
	// operation's scope that no goroutine owns.
	timers *Context
}

// Begin opens an operation on the calling goroutine, which owns its context
// from then on.
func Begin() *Scope {
	c := (*Context)(getContext())
	if c == nil {
		rootSaved = exchangeImport(0, 0)
		c = &Context{saved: none}
		setContext(unsafe.Pointer(c))
	}
	s := &Scope{c: c, outer: c.op}
	c.op = &Operation{}
	s.dom = domainEnter(nil)
	return s
}

// Entered records that the operation's live scope is installed on the calling
// goroutine.
func Entered(s *Scope) {
	s.timers = &Context{saved: exchangeImport(0, 0), op: s.c.op}
	domainSetContext(s.dom, unsafe.Pointer(s.timers))
}

// Join waits until every goroutine started inside the operation has exited,
// then stops the operation's timers and ends their host clock waits while the
// operation's scope is still open.
func Join(s *Scope) {
	if op := s.c.op; op.live > 0 {
		op.done = make(chan struct{})
		<-op.done
	}
	domainStop(s.dom)
	witAsync.CancelClockWaits(s.dom)
	if s.timers != nil {
		release(s.timers)
	}
}

// End closes an operation opened by Begin. A goroutine outside every operation
// runs in the root context again.
func End(s *Scope) {
	domainExit(s.dom)
	c := s.c
	c.op = s.outer
	if s.outer == nil {
		installContext(nil)
		release(c)
		setContext(nil)
	}
}

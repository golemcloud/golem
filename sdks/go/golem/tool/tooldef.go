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

package tool

import (
	"errors"
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"io"
	"reflect"
	"slices"
	"strings"

	"github.com/golemcloud/golem/sdks/go/core/values"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool definitions.
//
// A tool is a command tree declared once, from which the SDK derives the
// metadata the host publishes, the dispatch of incoming invocations and typed
// calls. The tool's name is its root command:
//
//	var Git = tool.DefineTool("git", tool.Spec{Version: "1.0.0", Summary: "A tiny git"}).
//	    Globals(func(g *GitGlobals, s *tool.GlobalsSpec) {
//	        s.Option(&g.Dir).Short('C').Default(".")
//	    })
//
//	var Commit = Git.Command[CommitArgs, CommitResult]("commit", func(a *CommitArgs, s *tool.CommandSpec) {
//	    s.Doc("Record changes")
//	    s.Option(&a.Message).Short('m')
//	})
//
//	var _ = Commit.Handle(func(ctx *tool.Context, a CommitArgs) (CommitResult, error) {
//	    return commit(a.Dir, a.Message)
//	})
//
// and a typed call, from any agent of any component that can reach the tool:
//
//	res, err := Commit.Call(func(a *CommitArgs) { a.Message = "fix" })

// Spec describes a tool as a whole.
type Spec struct {
	// Version is the tool's own version, reported in its metadata.
	Version string
	// Summary is the one-line description shown in help output.
	Summary string
	// Description is the longer prose shown for the tool itself.
	Description string
	// Aliases are additional names the root command answers to.
	Aliases []string
	// RequiresFilesystem declares that the tool cannot work without a
	// filesystem binding. It does not grant filesystem access.
	RequiresFilesystem bool
}

// toolEntry is one tool: defined here and exported, or declared for calling.
type toolEntry struct {
	name string
	spec Spec
	// remote marks a tool declared only to be called, whose implementation
	// lives elsewhere; it is never exported and takes no handlers.
	remote       bool
	r            *toolRegistry
	d            *definitions
	root         *toolNode
	errorsByName map[string]*toolErrorInfo
	// built caches the derived metadata, which cannot change once the
	// component has initialized; a declaration clears it.
	built   *toolCommon.Tool
	builtOK bool
}

func (e *toolEntry) fail(format string, args ...any) {
	e.d.RecordErr("", "", "tool %s: %s", e.name, fmt.Sprintf(format, args...))
}

func (e *toolEntry) changed() { e.built = nil }

// toolNode is one command of the tree: a group, a command, or both.
type toolNode struct {
	entry    *toolEntry
	parent   *toolNode
	path     []string
	name     string
	doc      toolDoc
	aliases  []string
	globals  *globalsDecl
	children []*toolNode
	body     *commandEntry
}

func (n *toolNode) label() string { return commandLabel(n.path) }

// find walks a command path from n, matching each segment against a child's
// name or aliases, as the host does.
func (n *toolNode) find(path []string) *toolNode {
	at := n
	for _, seg := range path {
		var next *toolNode
		for _, c := range at.children {
			if c.name == seg || slices.Contains(c.aliases, seg) {
				next = c
				break
			}
		}
		if next == nil {
			return nil
		}
		at = next
	}
	return at
}

func (n *toolNode) child(name string) *toolNode {
	c := &toolNode{entry: n.entry, parent: n, path: append(slices.Clone(n.path), name), name: name}
	if name == "" {
		n.entry.fail("a command under %s needs a name", n.label())
		return c
	}
	for _, existing := range n.children {
		if existing.name == name {
			n.entry.fail("command already declared: %s", c.label())
			return c
		}
	}
	n.children = append(n.children, c)
	n.entry.changed()
	return c
}

// globalsDecl is a node's global options and flags, bound to the fields of G.
type globalsDecl struct {
	typ      reflect.Type
	state    specState
	resolved bool
	args     []boundArg
	ok       bool
}

// Definition is a tool, returned by [DefineTool] or [DefineToolClient]. It
// is the tool's root command, so commands, groups and globals are declared on it
// directly: every [Group] method applies to it.
//
// T is the tool's identity type, an empty struct of its own, as an agent is
// identified by its Id type. Commands, error cases, clients and middleware
// carry it, so one tool's command cannot be used where another's is expected.
type Definition[T any] struct {
	*rootGroup[T]
	entry *toolEntry
}

// rootGroup names the embedded root group, so that its field does not hide
// the Group method it promotes.
type rootGroup[T any] = Group[T]

// Name returns the tool's name, which is also its root command name.
func (t *Definition[T]) Name() string { return t.entry.name }

// DefineTool declares a tool this component implements and exports. Call it
// from a package-level var so the declaration happens before the component is
// invoked.
func DefineTool[T any](name string, spec Spec) *Definition[T] {
	return defineToolInto[T](toolDefs, defs, name, spec, false)
}

// DefineToolClient declares the shape of a tool to call with typed arguments,
// without implementing it here: a tool of another component, a remote release
// or an imported MCP server. The name is the one the tool is looked up by in
// the environment. A tool client is never exported, and its commands take no
// handlers.
func DefineToolClient[T any](name string) *Definition[T] {
	return defineToolInto[T](toolDefs, defs, name, Spec{}, true)
}

// Client is a target for a tool's commands: the tool registered under
// another name than the definition's, with the same shape.
type Client[T any] struct{ name string }

// Name returns the registration name calls through this client go to.
func (c *Client[T]) Name() string { return c.name }

// Bind targets the tool registered under name, which must have this
// definition's shape. Use it with a command's On.
func (t *Definition[T]) Bind(name string) *Client[T] { return &Client[T]{name: name} }

func defineToolInto[T any](r *toolRegistry, d *definitions, name string, spec Spec, remote bool) *Definition[T] {
	e := &toolEntry{name: name, spec: spec, remote: remote, r: r, d: d, errorsByName: map[string]*toolErrorInfo{}}
	e.root = &toolNode{
		entry:   e,
		name:    name,
		doc:     toolDoc{summary: spec.Summary, description: spec.Description},
		aliases: slices.Clone(spec.Aliases),
	}
	t := &Definition[T]{rootGroup: &Group[T]{node: e.root}, entry: e}
	switch {
	case name == "":
		d.RecordErr("", "", "DefineTool requires a name")
	case r.byName[name] != nil || r.remote[name] != nil:
		d.RecordErr("", "", "tool already defined: %s", name)
	case remote:
		r.remote[name] = e
	default:
		r.order = append(r.order, name)
		r.byName[name] = e
	}
	return t
}

// Group is a command that dispatches to subcommands: the tool's root, or a
// group declared under it. A group may also have a body of its own.
type Group[T any] struct{ node *toolNode }

// Path returns the group's path from the tool's root; empty for the root.
func (g *Group[T]) Path() []string { return slices.Clone(g.node.path) }

// Doc sets the group's one-line summary.
func (g *Group[T]) Doc(summary string) *Group[T] { g.node.doc.summary = summary; return g }

// Description sets the group's longer description.
func (g *Group[T]) Description(text string) *Group[T] {
	g.node.doc.description = text
	return g
}

// Example adds a usage example to the group's documentation.
func (g *Group[T]) Example(title, body string) *Group[T] {
	g.node.doc.examples = append(g.node.doc.examples, toolCommon.Example{Title: title, Body: body})
	return g
}

// Aliases adds alternative names for the group.
func (g *Group[T]) Aliases(names ...string) *Group[T] {
	g.node.aliases = append(g.node.aliases, names...)
	return g
}

// Group declares a group of subcommands under this one.
func (g *Group[T]) Group(name string) *Group[T] {
	return &Group[T]{node: g.node.child(name)}
}

// Globals declares options and flags that every command at or below this node
// inherits. A command's argument struct embeds G to receive them:
//
//	type GitGlobals struct{ Dir string }
//	type CommitArgs struct {
//	    GitGlobals
//	    Message string
//	}
func (g *Group[T]) Globals[G any](spec func(*G, *GlobalsSpec)) *Group[T] {
	n := g.node
	if n.globals != nil {
		n.entry.fail("%s declares its globals twice", n.label())
		return g
	}
	t := reflect.TypeFor[G]()
	ptr, target := newSpecTarget(t)
	s := &GlobalsSpec{state: specState{target: target}}
	if t.Kind() != reflect.Struct {
		n.entry.fail("the globals of %s are %s, but globals must be a struct", n.label(), t)
	} else if spec != nil {
		spec(ptr.Interface().(*G), s)
	}
	for _, msg := range s.state.errs {
		n.entry.fail("globals of %s: %s", n.label(), msg)
	}
	n.globals = &globalsDecl{typ: t, state: s.state}
	n.entry.changed()
	return g
}

// Command declares a subcommand of this group that returns a result.
func (g *Group[T]) Command[A any, O any](name string, spec func(*A, *CommandSpec)) *Command[T, A, O] {
	return &Command[T, A, O]{ce: declareBody[A, O](g.node.child(name), spec, false)}
}

// OutputCommand declares a subcommand that writes standard output, standard
// error or both besides returning its result; its spec declares which with
// Stdout and Stderr.
func (g *Group[T]) OutputCommand[A any, O any](name string, spec func(*A, *CommandSpec)) *OutputCommand[T, A, O] {
	return &OutputCommand[T, A, O]{ce: declareBody[A, O](g.node.child(name), spec, true)}
}

// Body declares what this group itself does when invoked without a
// subcommand; on the tool, what the tool does when invoked bare.
func (g *Group[T]) Body[A any, O any](spec func(*A, *CommandSpec)) *Command[T, A, O] {
	return &Command[T, A, O]{ce: declareBody[A, O](g.node, spec, false)}
}

// OutputBody is [Group.Body] for a body that writes standard output or
// standard error.
func (g *Group[T]) OutputBody[A any, O any](spec func(*A, *CommandSpec)) *OutputCommand[T, A, O] {
	return &OutputCommand[T, A, O]{ce: declareBody[A, O](g.node, spec, true)}
}

// commandEntry is one command body: its argument spec, its handler, and the
// layout resolved from both once every declaration has run.
type commandEntry struct {
	node     *toolNode
	argsType reflect.Type
	outType  reflect.Type
	// outputs marks a command declared with OutputCommand.
	outputs bool
	spec    *CommandSpec
	invoke  func(*OutputContext, reflect.Value) (reflect.Value, error)

	resolved bool
	layout   *commandLayout
}

func (ce *commandEntry) label() string { return ce.node.label() }

func declareBody[A any, O any](n *toolNode, spec func(*A, *CommandSpec), outputs bool) *commandEntry {
	ptr, target := newSpecTarget(reflect.TypeFor[A]())
	s := &CommandSpec{state: specState{target: target}, outputAllow: outputs}
	if spec != nil {
		spec(ptr.Interface().(*A), s)
	}
	if outputs && s.settings.stdout == nil && s.settings.stderr == nil {
		s.state.fail("an OutputCommand declares no output; declare one with Stdout or Stderr, or use Command")
	}
	ce := &commandEntry{
		node: n, argsType: reflect.TypeFor[A](), outType: reflect.TypeFor[O](), outputs: outputs, spec: s,
	}
	e := n.entry
	for _, msg := range s.state.errs {
		e.fail("command %s: %s", n.label(), msg)
	}
	if n.body != nil {
		e.fail("command %s already has a body", n.label())
		return ce
	}
	if d := s.settings.doc; d.summary != "" || d.description != "" || len(d.examples) > 0 {
		n.doc = d
	}
	n.aliases = append(n.aliases, s.settings.aliases...)
	n.body = ce
	e.changed()
	return ce
}

func (ce *commandEntry) setHandler(h func(*OutputContext, reflect.Value) (reflect.Value, error)) golem.Registered {
	e := ce.node.entry
	switch {
	case e.remote:
		e.fail("command %s belongs to a remote tool and cannot be handled here", ce.label())
	case ce.node.body != ce:
		// A body rejected at declaration is not part of the tree.
	case ce.invoke != nil:
		e.fail("command %s already has a handler", ce.label())
	default:
		ce.invoke = h
		e.changed()
	}
	return golem.Registered{}
}

// Command is a declared command that returns a result.
type Command[T any, A any, O any] struct {
	ce *commandEntry
	// target overrides the registration name a call goes to; see On.
	target string
}

// Path returns the command's path from the tool's root.
func (c *Command[T, A, O]) Path() []string { return slices.Clone(c.ce.node.path) }

// Handle binds the command's implementation. A declared error is returned as
// ErrX.New(payload); any other error fails the invocation. Call it from a
// package-level var so the binding happens before the component is invoked.
func (c *Command[T, A, O]) Handle(h func(*Context, A) (O, error)) golem.Registered {
	return c.ce.setHandler(func(ctx *OutputContext, args reflect.Value) (reflect.Value, error) {
		out, err := h(&ctx.Context, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// OutputCommand is a declared command that writes standard output or
// standard error besides returning a result.
type OutputCommand[T any, A any, O any] struct {
	ce     *commandEntry
	target string
}

// Path returns the command's path from the tool's root.
func (c *OutputCommand[T, A, O]) Path() []string { return slices.Clone(c.ce.node.path) }

// Handle binds the command's implementation, which writes its outputs through
// [OutputContext.Stdout] and [OutputContext.Stderr].
func (c *OutputCommand[T, A, O]) Handle(h func(*OutputContext, A) (O, error)) golem.Registered {
	return c.ce.setHandler(func(ctx *OutputContext, args reflect.Value) (reflect.Value, error) {
		out, err := h(ctx, args.Interface().(A))
		return reflect.ValueOf(&out).Elem(), err
	})
}

// boundArg is a binding placed in a command's argument struct.
type boundArg struct {
	b *argBinding
	// path addresses the field within the command's argument struct, through
	// the globals embedding for an inherited global.
	path []int
	// field types the field as the canonical input record carries it, and
	// value types what the metadata declares: the element of a list, the
	// inner type of an optional field.
	field  *engine.Codec
	value  *engine.Codec
	global bool
}

// commandLayout is a command's canonical input record and everything derived
// with it.
type commandLayout struct {
	// fields is the canonical record, in order: inherited globals root first
	// (options before flags per node), positionals, the tail, options, flags.
	fields      []boundArg
	stdin       *stdinBinding
	principal   []int
	defaults    reflect.Value
	constraints []toolCommon.Constraint
	formatters  []toolCommon.Formatter
	defaultFmt  string
}

// argOrder is the canonical position of a binding kind within one group of
// fields.
func argOrder(k argKind) int {
	switch k {
	case argPositional:
		return 0
	case argTail:
		return 1
	case argOption, argList, argMap:
		return 2
	default:
		return 3
	}
}

func (d *definitions) boundArgOf(b *argBinding, prefix []int) (boundArg, error) {
	a := boundArg{
		b:     b,
		path:  append(slices.Clone(prefix), b.path...),
		field: d.Compile(b.field.Type),
		value: d.Compile(b.value),
	}
	if a.field.Invalid != "" {
		return a, fmt.Errorf("%s is %s, which cannot be represented: %s", b.name, b.field.Type, a.field.Invalid)
	}
	if a.value.Invalid != "" {
		return a, fmt.Errorf("%s is %s, which cannot be represented: %s", b.name, b.value, a.value.Invalid)
	}
	if b.restrictErr != nil {
		return a, fmt.Errorf("%s: %w", b.name, b.restrictErr)
	}
	if b.restrict != nil {
		if err := d.CheckRestriction(a.value, b.restrict); err != nil {
			return a, fmt.Errorf("%s: %w", b.name, err)
		}
	}
	if b.optional && b.def.IsValid() {
		return a, fmt.Errorf("%s is optional (golem.Option) and cannot also have a default; use a plain type with Default", b.name)
	}
	return a, nil
}

// resolve places a node's globals. It runs once.
func (gd *globalsDecl) resolve(n *toolNode) ([]boundArg, bool) {
	if gd.resolved {
		return gd.args, gd.ok
	}
	gd.resolved, gd.ok = true, len(gd.state.errs) == 0 && gd.typ.Kind() == reflect.Struct
	bindings := slices.Clone(gd.state.bindings)
	slices.SortStableFunc(bindings, func(a, b *argBinding) int { return argOrder(a.kind) - argOrder(b.kind) })
	for _, b := range bindings {
		a, err := n.entry.d.boundArgOf(b, nil)
		if err != nil {
			n.entry.fail("globals of %s: %v", n.label(), err)
			gd.ok = false
			continue
		}
		a.global = true
		gd.args = append(gd.args, a)
	}
	return gd.args, gd.ok
}

// resolve places a command's arguments. It runs once, lazily, because the
// globals of the nodes above may be declared after the command itself.
func (ce *commandEntry) resolve() (*commandLayout, bool) {
	if ce.resolved {
		return ce.layout, ce.layout != nil
	}
	ce.resolved = true
	e, d := ce.node.entry, ce.node.entry.d
	var errs []string
	fail := func(format string, args ...any) { errs = append(errs, fmt.Sprintf(format, args...)) }
	defer func() {
		for _, msg := range errs {
			e.fail("command %s: %s", ce.label(), msg)
		}
	}()

	t := ce.argsType
	if t.Kind() != reflect.Struct {
		fail("takes %s, but command arguments must be a struct", t)
		return nil, false
	}
	if len(ce.spec.state.errs) > 0 {
		return nil, false
	}
	l := &commandLayout{stdin: ce.spec.state.stdin}

	var chain []*toolNode
	for n := ce.node; n != nil; n = n.parent {
		chain = append([]*toolNode{n}, chain...)
	}
	var embeds [][]int
	for _, n := range chain {
		if n.globals == nil {
			continue
		}
		args, ok := n.globals.resolve(n)
		if !ok {
			fail("the globals of %s are not well-defined", n.label())
			continue
		}
		embed, found := embeddedPath(t, n.globals.typ)
		if !found {
			fail("its arguments %s must embed %s, the globals of %s", t, n.globals.typ, n.label())
			continue
		}
		embeds = append(embeds, embed)
		for _, a := range args {
			a.path = append(slices.Clone(embed), a.b.path...)
			l.fields = append(l.fields, a)
		}
	}

	local := slices.Clone(ce.spec.state.bindings)
	slices.SortStableFunc(local, func(a, b *argBinding) int { return argOrder(a.kind) - argOrder(b.kind) })
	tails, optionalPositional := 0, ""
	for _, b := range local {
		if underAny(b.path, embeds) {
			fail("field %s belongs to the embedded globals; bind it in Globals", b.field.Name)
			continue
		}
		switch b.kind {
		case argTail:
			tails++
		case argPositional:
			required := !b.optional && !b.def.IsValid()
			if required && optionalPositional != "" && !e.remote {
				fail("the required positional %s follows the optional positional %s", b.name, optionalPositional)
			}
			if !required && optionalPositional == "" {
				optionalPositional = b.name
			}
		}
		a, err := d.boundArgOf(b, nil)
		if err != nil {
			fail("%v", err)
			continue
		}
		l.fields = append(l.fields, a)
	}
	if tails > 1 {
		fail("binds %d tails; a command has at most one", tails)
	}

	bound := map[string]bool{}
	for _, a := range l.fields {
		bound[pathString(a.path)] = true
	}
	if l.stdin != nil {
		bound[pathString(l.stdin.path)] = true
	}
	principalType := reflect.TypeFor[golem.Principal]()
	var walk func(t reflect.Type, prefix []int)
	walk = func(t reflect.Type, prefix []int) {
		for i := range t.NumField() {
			f := t.Field(i)
			path := append(slices.Clone(prefix), i)
			switch {
			case bound[pathString(path)]:
			case f.Anonymous && f.Type.Kind() == reflect.Struct:
				if !slices.ContainsFunc(embeds, func(e []int) bool { return slices.Equal(e, path) }) {
					walk(f.Type, path)
				}
			case !f.IsExported():
			case f.Type == principalType:
				if l.principal != nil {
					fail("has more than one golem.Principal field")
				}
				l.principal = path
			default:
				fail("field %s is not bound; bind it in the spec, or unexport it", f.Name)
			}
		}
	}
	walk(t, nil)

	l.defaults = reflect.New(t).Elem()
	for _, a := range l.fields {
		fv := l.defaults.FieldByIndex(a.path)
		switch {
		case a.b.def.IsValid():
			fv.Set(a.b.def)
		case a.b.kind == argFlag && a.b.flagDefault:
			fv.SetBool(true)
		}
	}

	l.constraints = ce.buildConstraints(l, fail)
	l.formatters, l.defaultFmt = ce.buildFormatters(fail)
	for _, info := range ce.spec.settings.raises {
		if info.tool != e.name {
			fail("raises %s, an error declared on the tool %s", info.name, info.tool)
		}
	}

	if len(errs) > 0 {
		return nil, false
	}
	ce.layout = l
	return l, true
}

// embeddedPath finds the embedded field of type g, directly or through other
// embedded structs.
func embeddedPath(t reflect.Type, g reflect.Type) ([]int, bool) {
	type item struct {
		t    reflect.Type
		path []int
	}
	queue := []item{{t, nil}}
	for len(queue) > 0 {
		it := queue[0]
		queue = queue[1:]
		for i := range it.t.NumField() {
			f := it.t.Field(i)
			if !f.Anonymous || f.Type.Kind() != reflect.Struct {
				continue
			}
			path := append(slices.Clone(it.path), i)
			if f.Type == g {
				return path, true
			}
			queue = append(queue, item{f.Type, path})
		}
	}
	return nil, false
}

func underAny(path []int, prefixes [][]int) bool {
	for _, p := range prefixes {
		if len(path) >= len(p) && slices.Equal(path[:len(p)], p) {
			return true
		}
	}
	return false
}

func pathString(path []int) string { return fmt.Sprint(path) }

func (ce *commandEntry) buildConstraints(l *commandLayout, fail func(string, ...any)) []toolCommon.Constraint {
	d := ce.node.entry.d
	ref := func(r refDecl) toolCommon.Ref {
		var a *boundArg
		for i := range l.fields {
			f := &l.fields[i]
			if (r.b != nil && f.b == r.b) || (r.b == nil && r.path != nil && slices.Equal(f.path, r.path)) {
				a = f
				break
			}
		}
		if a == nil {
			fail("a constraint refers to a field that is not bound as an argument")
			return toolCommon.MakeRefPresent("")
		}
		if !r.value.IsValid() {
			return toolCommon.MakeRefPresent(a.b.name)
		}
		if a.b.kind.isFlag() {
			fail("a constraint compares the flag %s with a value; refer to a flag as present", a.b.name)
			return toolCommon.MakeRefPresent(a.b.name)
		}
		v, c := r.value, a.value
		if a.b.kind == argMap {
			c = d.Compile(a.b.value.Elem())
		}
		if v.Type() != c.Typ && a.b.optional && v.Type() == a.b.field.Type {
			inner, some, _ := values.OptionGet(v.Interface())
			if !some {
				fail("a constraint compares %s with None; refer to it as present instead", a.b.name)
				return toolCommon.MakeRefPresent(a.b.name)
			}
			v = inner
		}
		if v.Type() != c.Typ {
			fail("a constraint compares %s, of %s, with a %s value", a.b.name, c.Typ, v.Type())
			return toolCommon.MakeRefPresent(a.b.name)
		}
		return toolCommon.MakeRefValueIs(toolCommon.ValueIsRef{Name: a.b.name, Value: engine.EncodeWith(c, v)})
	}
	refs := func(in []refDecl) []toolCommon.Ref {
		out := make([]toolCommon.Ref, 0, len(in))
		for _, r := range in {
			out = append(out, ref(r))
		}
		return out
	}

	out := make([]toolCommon.Constraint, 0, len(ce.spec.constraints))
	for _, c := range ce.spec.constraints {
		switch c.kind {
		case constraintRequiresAll:
			out = append(out, toolCommon.MakeConstraintRequiresAll(refs(c.refs)))
		case constraintAllOrNone:
			out = append(out, toolCommon.MakeConstraintAllOrNone(refs(c.refs)))
		case constraintRequiresAny:
			out = append(out, toolCommon.MakeConstraintRequiresAny(refs(c.refs)))
		case constraintMutexGroups:
			groups := make([]toolCommon.RefGroup, 0, len(c.groups))
			for _, g := range c.groups {
				groups = append(groups, toolCommon.RefGroup{Refs: refs(g)})
			}
			out = append(out, toolCommon.MakeConstraintMutexGroups(groups))
		case constraintImplies:
			out = append(out, toolCommon.MakeConstraintImplies(toolCommon.ImpliesC{
				LhsQuant: c.lhs.quant, Lhs: refs(c.lhs.refs),
				RhsQuant: c.rhs.quant, Rhs: refs(c.rhs.refs),
			}))
		case constraintForbids:
			out = append(out, toolCommon.MakeConstraintForbids(toolCommon.ForbidsC{
				LhsQuant: c.lhs.quant, Lhs: refs(c.lhs.refs), Rhs: refs(c.rhs.refs),
			}))
		}
	}
	return out
}

// implicitFormatter names the formatter of a result that declares none. A
// result always renders somehow, and the WIT requires the default to name a
// declared formatter, so it is published as one formatter of that name, as the
// other SDKs do.
const implicitFormatter = "default"

func (ce *commandEntry) buildFormatters(fail func(string, ...any)) ([]toolCommon.Formatter, string) {
	st := ce.spec.settings
	if ce.outType == reflect.TypeFor[golem.Unit]() {
		if len(st.formatters) > 0 || st.defaultFormatter != "" {
			fail("declares formatters but returns no result")
		}
		return nil, ""
	}
	if len(st.formatters) == 0 {
		if st.defaultFormatter != "" && st.defaultFormatter != implicitFormatter {
			fail("defaults to the formatter %q, which it does not declare", st.defaultFormatter)
		}
		return []toolCommon.Formatter{{Name: implicitFormatter, Doc: toolDoc{}.toWit()}}, implicitFormatter
	}
	seen := map[string]bool{}
	for _, f := range st.formatters {
		switch {
		case f.Name == "":
			fail("declares a formatter with no name")
		case seen[f.Name]:
			fail("declares the formatter %q twice", f.Name)
		}
		seen[f.Name] = true
	}
	chosen := st.defaultFormatter
	if chosen == "" {
		chosen = st.formatters[0].Name
	} else if !seen[chosen] {
		fail("defaults to the formatter %q, which it does not declare", chosen)
	}
	return slices.Clone(st.formatters), chosen
}

// buildTool derives the metadata the host discovers for one tool.
func (d *definitions) buildTool(e *toolEntry) (toolCommon.Tool, bool) {
	if e.built != nil {
		return *e.built, e.builtOK
	}
	g := engine.GraphBuilder{E: d.Engine}
	ok := true
	var nodes []toolCommon.CommandNode
	var walk func(n *toolNode) int32
	walk = func(n *toolNode) int32 {
		idx := int32(len(nodes))
		nodes = append(nodes, toolCommon.CommandNode{})
		cn := toolCommon.CommandNode{
			Name:    n.name,
			Aliases: slices.Clone(n.aliases),
			Doc:     n.doc.toWit(),
			Body:    witTypes.None[toolCommon.CommandBody](),
		}
		if n.globals != nil {
			args, gok := n.globals.resolve(n)
			ok = ok && gok
			for _, a := range args {
				if a.b.kind.isOption() {
					cn.Globals.Options = append(cn.Globals.Options, optionSpecOf(&g, a))
				} else {
					cn.Globals.Flags = append(cn.Globals.Flags, flagSpecOf(a))
				}
			}
		}
		if ce := n.body; ce != nil {
			if ce.invoke == nil && !e.remote {
				e.fail("command %s has no handler; call Handle on it", ce.label())
				ok = false
			}
			if l, lok := ce.resolve(); lok {
				cn.Body = witTypes.Some(d.buildCommandBody(&g, ce, l))
			} else {
				ok = false
			}
		}
		for _, c := range n.children {
			cn.Subcommands = append(cn.Subcommands, walk(c))
		}
		nodes[idx] = cn
		return idx
	}
	walk(e.root)

	for typ, why := range g.Invalids {
		e.fail("references %s, which cannot be represented: %s", typ, why)
		ok = false
	}
	tool := toolCommon.Tool{
		Version:            e.spec.Version,
		RequiresFilesystem: e.spec.RequiresFilesystem,
		Commands:           toolCommon.CommandTree{Nodes: nodes},
		Schema:             g.Build(),
	}
	e.built, e.builtOK = &tool, ok
	return tool, ok
}

func someIfSet(s string) witTypes.Option[string] {
	if s == "" {
		return witTypes.None[string]()
	}
	return witTypes.Some(s)
}

func shortOf(r rune) witTypes.Option[rune] {
	if r == 0 {
		return witTypes.None[rune]()
	}
	return witTypes.Some(r)
}

func defaultOf(a boundArg) witTypes.Option[types.SchemaValueTree] {
	if !a.b.def.IsValid() {
		return witTypes.None[types.SchemaValueTree]()
	}
	return witTypes.Some(engine.EncodeWith(a.value, a.b.def))
}

func optionSpecOf(g *engine.GraphBuilder, a boundArg) toolCommon.OptionSpec {
	b := a.b
	var shape toolCommon.OptionShape
	required := false
	switch b.kind {
	case argList:
		shape = toolCommon.MakeOptionShapeRepeatableList(toolCommon.RepeatableListShape{
			Repetition: b.repetition, ItemType: g.RestrictedNode(a.value, b.restrict),
		})
	case argMap:
		policy := toolCommon.DuplicateKeyPolicyReject
		if b.lastKeyWins {
			policy = toolCommon.DuplicateKeyPolicyLastWins
		}
		shape = toolCommon.MakeOptionShapeRepeatableMap(toolCommon.RepeatableMapShape{
			Repetition: b.repetition, MapType: g.RestrictedNode(a.value, b.restrict), DuplicateKeyPolicy: policy,
		})
	default:
		if b.valueOptional {
			shape = toolCommon.MakeOptionShapeOptionalScalar(g.RestrictedNode(a.value, b.restrict))
		} else {
			shape = toolCommon.MakeOptionShapeScalar(g.RestrictedNode(a.value, b.restrict))
		}
		required = !b.optional && !b.def.IsValid()
	}
	return toolCommon.OptionSpec{
		Long:      b.name,
		Short:     shortOf(b.short),
		Aliases:   slices.Clone(b.aliases),
		Doc:       b.doc.toWit(),
		ValueName: someIfSet(b.valueName),
		Shape:     shape,
		Default:   defaultOf(a),
		Required:  required,
		EnvVar:    someIfSet(b.env),
	}
}

func flagSpecOf(a boundArg) toolCommon.FlagSpec {
	b := a.b
	shape := toolCommon.MakeFlagShapeBoolFlag(toolCommon.BoolFlagShape{Default: b.flagDefault, Negatable: b.negatable})
	if b.kind == argCount {
		max := witTypes.None[uint32]()
		if b.max != nil {
			max = witTypes.Some(*b.max)
		}
		shape = toolCommon.MakeFlagShapeCountFlag(max)
	}
	return toolCommon.FlagSpec{
		Long:    b.name,
		Short:   shortOf(b.short),
		Aliases: slices.Clone(b.aliases),
		Doc:     b.doc.toWit(),
		Shape:   shape,
		EnvVar:  someIfSet(b.env),
	}
}

func (d *definitions) buildCommandBody(g *engine.GraphBuilder, ce *commandEntry, l *commandLayout) toolCommon.CommandBody {
	body := toolCommon.CommandBody{
		Positionals: toolCommon.Positionals{Tail: witTypes.None[toolCommon.TailPositional]()},
		Constraints: l.constraints,
		Stdin:       witTypes.None[toolCommon.StreamSpec](),
		Stdout:      witTypes.None[toolCommon.StreamSpec](),
		Stderr:      witTypes.None[toolCommon.StreamSpec](),
		Result:      witTypes.None[toolCommon.ResultSpec](),
		Annotations: witTypes.None[toolCommon.CommandAnnotations](),
	}
	for _, a := range l.fields {
		if a.global {
			continue
		}
		b := a.b
		switch b.kind {
		case argPositional:
			body.Positionals.Fixed = append(body.Positionals.Fixed, toolCommon.Positional{
				Name:         b.name,
				Doc:          b.doc.toWit(),
				ValueName:    someIfSet(b.valueName),
				Type:         g.RestrictedNode(a.value, b.restrict),
				Default:      defaultOf(a),
				Required:     !b.optional && !b.def.IsValid(),
				AcceptsStdio: b.acceptsStdio,
			})
		case argTail:
			max := witTypes.None[uint32]()
			if b.max != nil {
				max = witTypes.Some(*b.max)
			}
			body.Positionals.Tail = witTypes.Some(toolCommon.TailPositional{
				Name:         b.name,
				Doc:          b.doc.toWit(),
				ValueName:    someIfSet(b.valueName),
				ItemType:     g.RestrictedNode(a.value, b.restrict),
				Min:          b.min,
				Max:          max,
				Separator:    someIfSet(b.separator),
				Verbatim:     b.verbatim,
				AcceptsStdio: b.acceptsStdio,
			})
		case argOption, argList, argMap:
			body.Options = append(body.Options, optionSpecOf(g, a))
		default:
			body.Flags = append(body.Flags, flagSpecOf(a))
		}
	}

	st := ce.spec.settings
	if l.stdin != nil {
		body.Stdin = witTypes.Some(toolCommon.StreamSpec{
			Doc: l.stdin.doc.toWit(), Mime: slices.Clone(l.stdin.mime), Required: !l.stdin.optional,
		})
	}
	if st.stdout != nil {
		body.Stdout = witTypes.Some(st.stdout.toWit())
	}
	if st.stderr != nil {
		body.Stderr = witTypes.Some(st.stderr.toWit())
	}
	if ce.outType != reflect.TypeFor[golem.Unit]() {
		body.Result = witTypes.Some(toolCommon.ResultSpec{
			Type:             g.Node(d.Compile(ce.outType)),
			Doc:              toolDoc{summary: st.resultDoc}.toWit(),
			Formatters:       l.formatters,
			DefaultFormatter: l.defaultFmt,
		})
	}
	for _, info := range st.raises {
		payload := witTypes.None[int32]()
		if info.payload != nil {
			payload = witTypes.Some(g.Node(d.Compile(info.payload)))
		}
		body.Errors = append(body.Errors, toolCommon.ErrorCase{
			Name:     info.name,
			Doc:      toolDoc{summary: info.spec.Summary, description: info.spec.Description}.toWit(),
			Kind:     uint8(info.spec.Kind),
			ExitCode: info.spec.ExitCode,
			Payload:  payload,
		})
	}
	if st.annotations != nil {
		body.Annotations = witTypes.Some(*st.annotations)
	}
	return body
}

// decode fills an argument struct from a canonical input record.
func (l *commandLayout) decode(tree types.SchemaValueTree, dst reflect.Value) error {
	dec := engine.Decoder{Nodes: tree.ValueNodes}
	if len(dec.Nodes) == 0 {
		if len(l.fields) == 0 {
			return nil
		}
		return fmt.Errorf("empty value tree but %d argument(s) expected", len(l.fields))
	}
	root, err := dec.Node(tree.Root)
	if err != nil {
		return err
	}
	if root.Tag() != types.SchemaValueNodeRecordValue {
		return fmt.Errorf("expected a record at the root of the input")
	}
	idxs := root.RecordValue()
	if len(idxs) != len(l.fields) {
		return fmt.Errorf("input record has %d field(s), want %d", len(idxs), len(l.fields))
	}
	for i, a := range l.fields {
		if err := a.field.Decode(&dec, dst.FieldByIndex(a.path), idxs[i]); err != nil {
			return fmt.Errorf("argument %q: %w", a.b.name, err)
		}
	}
	return nil
}

// encode renders an argument struct as the canonical input record, together
// with a graph rooted at the record's type, which is what the host checks the
// input against.
func (l *commandLayout) encode(d *definitions, args reflect.Value) types.TypedSchemaValue {
	g := engine.GraphBuilder{E: d.Engine}
	fields := make([]types.NamedFieldType, 0, len(l.fields))
	for _, a := range l.fields {
		fields = append(fields, types.NamedFieldType{Name: a.b.name, Body: g.RestrictedNode(a.field, a.b.restrict)})
	}
	g.Nodes = append(g.Nodes, types.SchemaTypeNode{Body: types.MakeSchemaTypeBodyRecordType(fields)})
	root := int32(len(g.Nodes) - 1)
	graph := g.Build()
	graph.Root = root

	var b engine.ValBuilder
	idxs := make([]int32, 0, len(l.fields))
	for _, a := range l.fields {
		idxs = append(idxs, a.field.Encode(&b, args.FieldByIndex(a.path)))
	}
	valueRoot := b.Push(types.MakeSchemaValueNodeRecordValue(idxs))
	return types.TypedSchemaValue{Graph: graph, Value: types.SchemaValueTree{ValueNodes: b.Nodes, Root: valueRoot}}
}

// invokeCommand runs one command: resolve it, decode the arguments, call the
// handler, and package the result as a self-contained typed value.
func (d *definitions) invokeCommand(
	e *toolEntry, commandPath []string, input types.TypedSchemaValue,
	stdin *byteReader, outs hostOutputs, principal golem.Principal,
) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	fail := witTypes.Err[toolCommon.InvocationResult, types.ToolError]
	n := e.root.find(commandPath)
	if n == nil || n.body == nil {
		return fail(types.MakeToolErrorInvalidCommandPath(slices.Clone(commandPath)))
	}
	ce := n.body
	stdout, stderr, terr := ce.outputsFor(outs)
	if terr == nil && ce.invoke == nil {
		def := toolDefinitionError(d)
		terr = &def
	}
	var args reflect.Value
	if terr == nil {
		args, terr = ce.decodeArgs(d, input, stdin, principal)
	}
	if terr != nil {
		refused := OutputFailed(fmt.Sprintf("command %s was refused before it ran", ce.label()))
		stdout.fail(refused)
		stderr.fail(refused)
		return fail(*terr)
	}
	ctx := &OutputContext{
		Context: Context{tool: e.name, path: slices.Clone(commandPath)}, stdout: stdout, stderr: stderr,
	}
	out, err := runWithOutputs(ce.label(), []*Output{stdout, stderr},
		func() (reflect.Value, error) { return ce.invoke(ctx, args) })
	if err != nil {
		return fail(d.handlerError(ce, err))
	}
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](d.encodeResult(ce, out))
}

// hostOutputs are the writers the host supplied for an invocation's standard
// output and standard error, nil where the caller attached none.
type hostOutputs struct{ stdout, stderr byteStreamSink }

// outputsFor pairs what the host supplied with what the command declares. A
// required output the caller did not attach refuses the call; an undeclared
// one fails a write, and is finished empty if the host supplied it anyway.
func (ce *commandEntry) outputsFor(outs hostOutputs) (stdout, stderr *Output, terr *types.ToolError) {
	pair := func(name string, decl *outputDecl, sink byteStreamSink) *Output {
		o := newToolOutput(name, sink)
		switch {
		case decl == nil:
			o.undeclared = fmt.Sprintf("golem: command %s declares no %s", ce.label(), name)
		case decl.required && sink == nil && terr == nil:
			e := types.MakeToolErrorInvalidInput(fmt.Sprintf("command %s requires its %s to be taken", ce.label(), name))
			terr = &e
		}
		return o
	}
	st := ce.spec.settings
	stdout = pair("stdout", st.stdout, outs.stdout)
	stderr = pair("stderr", st.stderr, outs.stderr)
	return stdout, stderr, terr
}

// decodeArgs fills a command's argument struct from an invocation: the
// canonical input record, the principal and standard input.
func (ce *commandEntry) decodeArgs(
	d *definitions, input types.TypedSchemaValue, stdin *byteReader, principal golem.Principal,
) (reflect.Value, *types.ToolError) {
	l, ok := ce.resolve()
	if !ok {
		e := toolDefinitionError(d)
		return reflect.Value{}, &e
	}
	args := reflect.New(ce.argsType).Elem()
	if err := l.decode(input.Value, args); err != nil {
		e := types.MakeToolErrorInvalidInput(err.Error())
		return reflect.Value{}, &e
	}
	if l.principal != nil && principal != nil {
		args.FieldByIndex(l.principal).Set(reflect.ValueOf(&principal).Elem())
	}
	if l.stdin != nil {
		switch {
		case stdin.present():
			args.FieldByIndex(l.stdin.path).Set(reflect.ValueOf(io.Reader(stdin)))
		case !l.stdin.optional:
			e := types.MakeToolErrorInvalidInput(fmt.Sprintf("command %s requires standard input", ce.label()))
			return reflect.Value{}, &e
		}
	}
	return args, nil
}

// encodeResult packages a command's result as a self-contained typed value.
func (d *definitions) encodeResult(ce *commandEntry, out reflect.Value) toolCommon.InvocationResult {
	res := toolCommon.InvocationResult{
		Result: witTypes.None[types.TypedSchemaValue](),
		Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
		Stderr: witTypes.None[*witTypes.StreamReader[uint8]](),
	}
	if ce.outType != reflect.TypeFor[golem.Unit]() {
		c := d.Compile(ce.outType)
		g := engine.GraphBuilder{E: d.Engine}
		root := g.Node(c)
		graph := g.Build()
		graph.Root = root
		res.Result = witTypes.Some(types.TypedSchemaValue{Graph: graph, Value: engine.EncodeWith(c, out)})
	}
	return res
}

// handlerError turns what a handler failed with into the wire error: a
// declared case of the command, a failure of a tool it called passed through
// unchanged, or an invalid result.
func (d *definitions) handlerError(ce *commandEntry, err error) types.ToolError {
	var raised *RaisedError
	if errors.As(err, &raised) {
		return d.declaredToolError(ce, raised)
	}
	return passThroughToolError(err)
}

// passThroughToolError turns what a handler failed with into the wire error.
// A failure a called tool reported as its own error is kept unchanged; a
// rejection built with [InvalidInput] or [ConstraintViolation] keeps its kind;
// a failure of the call itself is mapped as the other SDKs map it — denial,
// cancellation and exhaustion become constraint violations, protocol and
// internal failures an invalid result; anything else is an invalid result.
func passThroughToolError(err error) types.ToolError {
	var call *CallError
	if !errors.As(err, &call) {
		return types.MakeToolErrorInvalidResult(err.Error())
	}
	if call.wire != nil {
		return *call.wire
	}
	switch call.Kind {
	case CallInvalidInput:
		return types.MakeToolErrorInvalidInput(call.Message)
	case CallConstraintViolation, CallDenied, CallResourceExhausted:
		return types.MakeToolErrorConstraintViolation(call.Message)
	case CallCancelled:
		return types.MakeToolErrorConstraintViolation("underlying invocation was cancelled")
	case CallProtocolError:
		return types.MakeToolErrorInvalidResult("protocol error: " + call.Message)
	case CallInternalError:
		return types.MakeToolErrorInvalidResult("internal error: " + call.Message)
	case CallUnknownTool:
		return types.MakeToolErrorInvalidToolName(call.Message)
	case CallUnknownCommand:
		return types.MakeToolErrorInvalidCommandPath(slices.Clone(call.CommandPath))
	}
	return types.MakeToolErrorInvalidResult(err.Error())
}

// runWithOutputs calls a handler, recovering a panic rather than letting it
// kill the component, and selects each output stream's terminal: finished when
// the handler succeeds, failed when it returns an error or panics. The wire
// accepts exactly one terminal and treats a dropped writer as abandoned, so
// choosing one here keeps a failing handler from looking like an abandoned
// transfer.
func runWithOutputs(label string, outputs []*Output, run func() (reflect.Value, error)) (out reflect.Value, err error) {
	defer func() {
		if r := recover(); r != nil {
			if re, ok := r.(*RaisedError); ok {
				err = re
			} else {
				err = fmt.Errorf("command %s panicked: %s", label, panicMessage(r))
			}
		}
		for _, o := range outputs {
			if o == nil {
				continue
			}
			if err != nil {
				o.fail(OutputFailed(err.Error()))
			} else if ferr := o.finish(); ferr != nil {
				err = ferr
			}
		}
	}()
	out, err = run()
	if err != nil {
		var raised *RaisedError
		var call *CallError
		if !errors.As(err, &raised) && !errors.As(err, &call) {
			err = fmt.Errorf("command %s failed: %w", label, err)
		}
	}
	return out, err
}

// declaredToolError turns a declared error case into the wire error. Returning
// a case the command did not list is itself a failure: the caller was handed a
// contract that does not mention it.
func (d *definitions) declaredToolError(ce *commandEntry, raised *RaisedError) types.ToolError {
	if !slices.Contains(ce.spec.settings.raises, raised.info) {
		return types.MakeToolErrorInvalidResult(fmt.Sprintf(
			"command %s returned the undeclared error %q; list it with Raises",
			ce.label(), raised.info.name))
	}
	// A case without a payload carries the empty tuple, which is what the host
	// checks such a case against.
	payload := types.TypedSchemaValue{
		Graph: types.SchemaGraph{
			TypeNodes: []types.SchemaTypeNode{{Body: types.MakeSchemaTypeBodyTupleType(nil)}},
		},
		Value: types.SchemaValueTree{ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeTupleValue(nil)}},
	}
	if raised.info.payload != nil {
		c := d.Compile(raised.info.payload)
		g := engine.GraphBuilder{E: d.Engine}
		root := g.Node(c)
		graph := g.Build()
		graph.Root = root
		payload = types.TypedSchemaValue{Graph: graph, Value: engine.EncodeWith(c, raised.payload)}
	}
	return types.MakeToolErrorCustomError(types.CustomToolError{Name: raised.info.name, Payload: payload})
}

// panicMessage renders a recovered panic for the stream failure reason.
func panicMessage(r any) string {
	if e, ok := r.(error); ok {
		return e.Error()
	}
	return fmt.Sprintf("%v", r)
}

func commandLabel(path []string) string {
	if len(path) == 0 {
		return "<root>"
	}
	return strings.Join(path, " ")
}

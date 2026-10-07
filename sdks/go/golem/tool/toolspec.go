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
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"io"
	"reflect"
	"slices"
	"strings"
	"unicode"

	"github.com/golemcloud/golem/sdks/go/core/values"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
)

// Tool argument specs.
//
// A command's arguments are an ordinary Go struct. Its spec function binds each
// field to the command line, once, when the command is declared:
//
//	type CommitArgs struct {
//	    GitGlobals                          // the tool's global options
//	    Message string                      // required: a plain type without a default
//	    Author  golem.Option[string]        // optional
//	    Amend   bool
//	    Output  OutputMode
//	}
//
//	var Commit = Git.Command[CommitArgs, CommitResult]("commit", func(a *CommitArgs, s *tool.CommandSpec) {
//	    s.Option(&a.Message).Short('m').Aliases("msg")
//	    author := s.Option(&a.Author).Env("GIT_AUTHOR_NAME")
//	    amend := s.Flag(&a.Amend).Negatable()
//	    output := s.Option(&a.Output).Default(OutputHuman)
//	    s.Implies(amend, author)
//	    s.Forbids(output.ValueIs(OutputJSON), amend)
//	})
//
// The field's type decides the shape: a plain T is required unless it has a
// default, golem.Option[T] is optional, a slice is a list (or the tail), a map
// is a key-value option, a bool is a flag and a uint32 is a count flag. The wire
// name is the field name in kebab case, unless Name overrides it.

// argKind separates the places a bound field can occupy on the command line.
type argKind uint8

const (
	argPositional argKind = iota
	argTail
	argOption
	argList
	argMap
	argFlag
	argCount
)

func (k argKind) isOption() bool { return k == argOption || k == argList || k == argMap }
func (k argKind) isFlag() bool   { return k == argFlag || k == argCount }

// toolDoc is the documentation attached to a command, a group or an argument.
type toolDoc struct {
	summary     string
	description string
	examples    []toolCommon.Example
}

func (d toolDoc) toWit() toolCommon.Doc {
	return toolCommon.Doc{
		Summary:     d.summary,
		Description: d.description,
		Examples:    append([]toolCommon.Example{}, d.examples...),
	}
}

// argBinding is one bound field and everything declared about it.
type argBinding struct {
	kind argKind
	name string
	// path is the field's index path within the struct the spec was run on.
	path  []int
	field reflect.StructField
	// value is the type the command line carries: the field type, the inner
	// type of an optional field, a list's element, or the map type.
	value reflect.Type
	// optional marks a golem.Option field: optional, and without a default.
	optional bool
	// restrict narrows the values the argument accepts; restrictErr is a
	// setter's mistake, reported when the tool is defined.
	restrict    *engine.Restriction
	restrictErr error

	doc           toolDoc
	valueName     string
	short         rune
	aliases       []string
	env           string
	def           reflect.Value
	valueOptional bool
	repetition    toolCommon.Repetition
	lastKeyWins   bool
	acceptsStdio  bool
	min           uint32
	max           *uint32
	separator     string
	verbatim      bool
	negatable     bool
	flagDefault   bool
}

// stdinBinding is the field a command reads standard input from.
type stdinBinding struct {
	path     []int
	doc      toolDoc
	mime     []string
	optional bool
}

// specTarget resolves pointers handed to a spec function back to the fields of
// the struct the function was run on.
type specTarget struct {
	typ    reflect.Type
	fields map[fieldKey][]int
}

type fieldKey struct {
	addr uintptr
	typ  reflect.Type
}

// newSpecTarget allocates the struct a spec function runs on and indexes its
// fields by address and type. Embedded structs are indexed through, so a field
// of an embedded globals struct resolves too; the address alone would not be
// enough, since an embedded struct and its first field share one.
func newSpecTarget(t reflect.Type) (reflect.Value, *specTarget) {
	ptr := reflect.New(t)
	st := &specTarget{typ: t, fields: map[fieldKey][]int{}}
	var walk func(v reflect.Value, prefix []int)
	walk = func(v reflect.Value, prefix []int) {
		for i := range v.NumField() {
			f := v.Type().Field(i)
			if !f.IsExported() && !f.Anonymous {
				continue
			}
			path := append(append([]int{}, prefix...), i)
			fv := v.Field(i)
			st.fields[fieldKey{fv.Addr().Pointer(), f.Type}] = path
			if f.Anonymous && f.Type.Kind() == reflect.Struct {
				walk(fv, path)
			}
		}
	}
	if t.Kind() == reflect.Struct {
		walk(ptr.Elem(), nil)
	}
	return ptr, st
}

// resolve finds the field p points at, or reports why it cannot.
func (st *specTarget) resolve(p any) ([]int, reflect.StructField, error) {
	pv := reflect.ValueOf(p)
	if pv.Kind() != reflect.Pointer || pv.IsNil() {
		return nil, reflect.StructField{}, fmt.Errorf("expected a pointer to a field of %s", st.typ)
	}
	path, ok := st.fields[fieldKey{pv.Pointer(), pv.Type().Elem()}]
	if !ok {
		return nil, reflect.StructField{}, fmt.Errorf("the pointer does not address a field of %s", st.typ)
	}
	return path, st.typ.FieldByIndex(path), nil
}

// specState is what a spec function builds, shared by the command and globals
// specs.
type specState struct {
	target   *specTarget
	bindings []*argBinding
	stdin    *stdinBinding
	errs     []string
}

func (s *specState) fail(format string, args ...any) {
	s.errs = append(s.errs, fmt.Sprintf(format, args...))
}

// bind records a field binding; the returned binding is detached (still
// usable for chaining, never published) when the pointer does not resolve.
func (s *specState) bind(p any, kind argKind, value reflect.Type) *argBinding {
	path, field, err := s.target.resolve(p)
	b := &argBinding{kind: kind, value: value, repetition: toolCommon.MakeRepetitionRepeated()}
	if err != nil {
		s.fail("%v", err)
		return b
	}
	if field.Type.Size() == 0 {
		s.fail("field %s has a zero-size type, so it cannot be told apart from its neighbours", field.Name)
		return b
	}
	for _, other := range s.bindings {
		if samePath(other.path, path) {
			s.fail("field %s is bound twice", field.Name)
			return b
		}
	}
	b.path, b.field, b.name = path, field, kebab(field.Name)
	// A golem tag restricts the argument as it does a record field; setters
	// called on the binding afterwards add to it.
	if tag, ok := field.Tag.Lookup("golem"); ok {
		b.restrict, b.restrictErr = engine.ParseRestrictionTag(tag)
	}
	s.bindings = append(s.bindings, b)
	return b
}

func samePath(a, b []int) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// optionalInner reports the inner type of a golem.Option field.
func optionalInner(t reflect.Type) (reflect.Type, bool) {
	return values.OptionElem(reflect.New(t).Elem().Interface())
}

// CommandSpec declares a command: its arguments, bound to the fields of its
// argument struct, and its documentation, result and failure contract.
type CommandSpec struct {
	state       specState
	settings    commandSettings
	constraints []constraintDecl
	outputAllow bool
}

// commandSettings is everything a spec declares about the command itself.
type commandSettings struct {
	doc              toolDoc
	aliases          []string
	annotations      *toolCommon.CommandAnnotations
	resultDoc        string
	formatters       []toolCommon.Formatter
	defaultFormatter string
	raises           []*toolErrorInfo
	stdout           *outputDecl
	stderr           *outputDecl
}

// outputDecl is a declared standard output or standard error.
type outputDecl struct {
	doc      toolDoc
	mime     []string
	required bool
}

// GlobalsSpec declares a node's global options and flags, which every
// command below the node inherits. Globals are options and flags only.
type GlobalsSpec struct{ state specState }

// Positional binds a field to a positional argument.
func (s *CommandSpec) Positional[T any](p *T) *PositionalArg[T] {
	value, optional := reflect.TypeFor[T](), false
	if inner, ok := optionalInner(value); ok {
		value, optional = inner, true
	}
	b := s.state.bind(p, argPositional, value)
	b.optional = optional
	return newToolPositionalArg[T](b)
}

// Tail binds a slice field to the variadic positional after the fixed ones.
func (s *CommandSpec) Tail[T any](p *[]T) *TailArg[T] {
	return newToolTailArg[T](s.state.bind(p, argTail, reflect.TypeFor[T]()))
}

// Option binds a field to a named option carrying one value.
func (s *CommandSpec) Option[T any](p *T) *OptionArg[T] { return bindOption(&s.state, p) }

// Option binds a field to a global named option carrying one value.
func (s *GlobalsSpec) Option[T any](p *T) *OptionArg[T] { return bindOption(&s.state, p) }

func bindOption[T any](s *specState, p *T) *OptionArg[T] {
	value, optional := reflect.TypeFor[T](), false
	if inner, ok := optionalInner(value); ok {
		value, optional = inner, true
	}
	b := s.bind(p, argOption, value)
	b.optional = optional
	return newToolOptionArg[T](b)
}

// List binds a slice field to an option that may be given several times.
func (s *CommandSpec) List[T any](p *[]T) *ListArg[T] {
	return newToolListArg[T](s.state.bind(p, argList, reflect.TypeFor[T]()))
}

// List binds a slice field to a global option that may be given several times.
func (s *GlobalsSpec) List[T any](p *[]T) *ListArg[T] {
	return newToolListArg[T](s.state.bind(p, argList, reflect.TypeFor[T]()))
}

// Map binds a map field to a key-value option, given as key=value.
func (s *CommandSpec) Map[K comparable, V any](p *map[K]V) *MapArg[K, V] {
	return &MapArg[K, V]{b: s.state.bind(p, argMap, reflect.TypeFor[map[K]V]())}
}

// Map binds a map field to a global key-value option.
func (s *GlobalsSpec) Map[K comparable, V any](p *map[K]V) *MapArg[K, V] {
	return &MapArg[K, V]{b: s.state.bind(p, argMap, reflect.TypeFor[map[K]V]())}
}

// Flag binds a bool field to a switch.
func (s *CommandSpec) Flag(p *bool) *FlagArg {
	return &FlagArg{b: s.state.bind(p, argFlag, reflect.TypeFor[bool]())}
}

// Flag binds a bool field to a global switch.
func (s *GlobalsSpec) Flag(p *bool) *FlagArg {
	return &FlagArg{b: s.state.bind(p, argFlag, reflect.TypeFor[bool]())}
}

// CountFlag binds a uint32 field to a switch counted by repetition (-vvv).
func (s *CommandSpec) CountFlag(p *uint32) *CountFlagArg {
	return &CountFlagArg{b: s.state.bind(p, argCount, reflect.TypeFor[uint32]())}
}

// CountFlag binds a uint32 field to a global counted switch.
func (s *GlobalsSpec) CountFlag(p *uint32) *CountFlagArg {
	return &CountFlagArg{b: s.state.bind(p, argCount, reflect.TypeFor[uint32]())}
}

// Stdin binds an io.Reader field to the command's standard input. It is
// required unless marked Optional; a caller leaving a required one nil is
// refused before anything is sent.
func (s *CommandSpec) Stdin(p *io.Reader) *StdinArg {
	path, _, err := s.state.target.resolve(p)
	if err != nil {
		s.state.fail("Stdin: %v", err)
		return &StdinArg{b: &stdinBinding{}}
	}
	if s.state.stdin != nil {
		s.state.fail("a command has one standard input, and Stdin is called twice")
		return &StdinArg{b: &stdinBinding{}}
	}
	s.state.stdin = &stdinBinding{path: path}
	return &StdinArg{b: s.state.stdin}
}

// Doc sets the command's one-line summary.
func (s *CommandSpec) Doc(summary string) { s.settings.doc.summary = summary }

// Description sets the command's longer description.
func (s *CommandSpec) Description(text string) { s.settings.doc.description = text }

// Example adds a usage example to the command's documentation.
func (s *CommandSpec) Example(title, body string) {
	s.settings.doc.examples = append(s.settings.doc.examples, toolCommon.Example{Title: title, Body: body})
}

// Aliases adds alternative names for the command.
func (s *CommandSpec) Aliases(names ...string) {
	s.settings.aliases = append(s.settings.aliases, names...)
}

func (s *CommandSpec) annotations() *toolCommon.CommandAnnotations {
	if s.settings.annotations == nil {
		s.settings.annotations = &toolCommon.CommandAnnotations{}
	}
	return s.settings.annotations
}

// ReadOnly marks the command as not changing anything.
func (s *CommandSpec) ReadOnly() { s.annotations().ReadOnly = true }

// Destructive marks the command as possibly destroying data.
func (s *CommandSpec) Destructive() { s.annotations().Destructive = true }

// Idempotent marks the command as safe to repeat with the same arguments.
func (s *CommandSpec) Idempotent() { s.annotations().Idempotent = true }

// OpenWorld marks the command as reaching beyond the component, e.g. the network.
func (s *CommandSpec) OpenWorld() { s.annotations().OpenWorld = true }

// ResultDoc documents the command's result.
func (s *CommandSpec) ResultDoc(text string) { s.settings.resultDoc = text }

// Formatter declares a rendering the result is offered in, such as "json". The
// first declared is the default unless DefaultFormatter says otherwise; a
// result that declares none is offered as "default".
func (s *CommandSpec) Formatter(name, summary string) {
	s.settings.formatters = append(s.settings.formatters,
		toolCommon.Formatter{Name: name, Doc: toolDoc{summary: summary}.toWit()})
}

// Formatters declares several undocumented renderings at once.
func (s *CommandSpec) Formatters(names ...string) {
	for _, n := range names {
		s.Formatter(n, "")
	}
}

// DefaultFormatter picks the rendering used when the caller chooses none.
func (s *CommandSpec) DefaultFormatter(name string) { s.settings.defaultFormatter = name }

// Raises lists the declared errors the command may return.
func (s *CommandSpec) Raises(cases ...ErrorDef) {
	for _, c := range cases {
		s.settings.raises = append(s.settings.raises, c.toolErrorInfo())
	}
}

// Stdout declares the command's standard output, which the handler writes
// through [OutputContext.Stdout]. It is optional unless marked Required.
func (s *CommandSpec) Stdout() *OutputSpec { return s.output("Stdout", &s.settings.stdout) }

// Stderr declares the command's standard error, which the handler writes
// through [OutputContext.Stderr]. It is optional unless marked Required.
func (s *CommandSpec) Stderr() *OutputSpec { return s.output("Stderr", &s.settings.stderr) }

func (s *CommandSpec) output(method string, slot **outputDecl) *OutputSpec {
	if !s.outputAllow {
		s.state.fail("%s on a command without outputs; declare it with OutputCommand", method)
	}
	if *slot != nil {
		s.state.fail("%s is called twice", method)
	}
	*slot = &outputDecl{}
	return &OutputSpec{d: *slot}
}

func (o *outputDecl) toWit() toolCommon.StreamSpec {
	return toolCommon.StreamSpec{Doc: o.doc.toWit(), Mime: slices.Clone(o.mime), Required: o.required}
}

// OutputSpec is a declared output stream. Each setter returns it, so
// declarations chain.
type OutputSpec struct{ d *outputDecl }

// Doc documents what the stream carries.
func (o *OutputSpec) Doc(text string) *OutputSpec { o.d.doc.summary = text; return o }

// Description adds a longer explanation of the stream.
func (o *OutputSpec) Description(text string) *OutputSpec { o.d.doc.description = text; return o }

// Mime lists the media types the stream carries.
func (o *OutputSpec) Mime(types ...string) *OutputSpec {
	o.d.mime = append(o.d.mime, types...)
	return o
}

// Required makes the stream one every caller must take; a call without it is
// refused before the handler runs.
func (o *OutputSpec) Required() *OutputSpec { o.d.required = true; return o }

// Argument bindings. Each setter returns the binding, so declarations chain.

// PositionalArg is a field bound to a positional argument.
type PositionalArg[T any] struct {
	restrictable[*PositionalArg[T], T]
	b *argBinding
}

func newToolPositionalArg[T any](b *argBinding) *PositionalArg[T] {
	a := &PositionalArg[T]{b: b}
	a.restrictable = restrictable[*PositionalArg[T], T]{rb: b, self: a}
	return a
}

func (a *PositionalArg[T]) Name(name string) *PositionalArg[T] { a.b.name = name; return a }
func (a *PositionalArg[T]) Doc(summary string) *PositionalArg[T] {
	a.b.doc.summary = summary
	return a
}
func (a *PositionalArg[T]) Description(text string) *PositionalArg[T] {
	a.b.doc.description = text
	return a
}
func (a *PositionalArg[T]) ValueName(n string) *PositionalArg[T] { a.b.valueName = n; return a }

// AcceptsStdio lets the value be "-", read from standard input.
func (a *PositionalArg[T]) AcceptsStdio() *PositionalArg[T] {
	a.b.acceptsStdio = true
	return a
}

// Default makes the argument optional, taking v when it is left out.
func (a *PositionalArg[T]) Default(v T) *PositionalArg[T] {
	a.b.def = reflect.ValueOf(&v).Elem()
	return a
}

// ValueIs refers to the argument having the value v, for a constraint.
func (a *PositionalArg[T]) ValueIs(v T) Ref    { return valueRef(a.b, v) }
func (a *PositionalArg[T]) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *PositionalArg[T]) toolRefs() refsDecl { return single(a) }

// TailArg is a slice field bound to the variadic positional.
type TailArg[T any] struct {
	restrictable[*TailArg[T], T]
	b *argBinding
}

func newToolTailArg[T any](b *argBinding) *TailArg[T] {
	a := &TailArg[T]{b: b}
	a.restrictable = restrictable[*TailArg[T], T]{rb: b, self: a}
	return a
}

func (a *TailArg[T]) Name(name string) *TailArg[T]   { a.b.name = name; return a }
func (a *TailArg[T]) Doc(summary string) *TailArg[T] { a.b.doc.summary = summary; return a }
func (a *TailArg[T]) Description(text string) *TailArg[T] {
	a.b.doc.description = text
	return a
}
func (a *TailArg[T]) ValueName(n string) *TailArg[T]   { a.b.valueName = n; return a }
func (a *TailArg[T]) Min(n uint32) *TailArg[T]         { a.b.min = n; return a }
func (a *TailArg[T]) Max(n uint32) *TailArg[T]         { a.b.max = &n; return a }
func (a *TailArg[T]) AcceptsStdio() *TailArg[T]        { a.b.acceptsStdio = true; return a }
func (a *TailArg[T]) Separator(sep string) *TailArg[T] { a.b.separator = sep; return a }

// Verbatim takes everything after the separator as values, flags included.
func (a *TailArg[T]) Verbatim() *TailArg[T] { a.b.verbatim = true; return a }

// ValueIs refers to one of the values being v, for a constraint.
func (a *TailArg[T]) ValueIs(v T) Ref    { return valueRef(a.b, v) }
func (a *TailArg[T]) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *TailArg[T]) toolRefs() refsDecl { return single(a) }

// OptionArg is a field bound to a named option carrying one value.
type OptionArg[T any] struct {
	restrictable[*OptionArg[T], T]
	b *argBinding
}

func newToolOptionArg[T any](b *argBinding) *OptionArg[T] {
	a := &OptionArg[T]{b: b}
	a.restrictable = restrictable[*OptionArg[T], T]{rb: b, self: a}
	return a
}

func (a *OptionArg[T]) Name(name string) *OptionArg[T]   { a.b.name = name; return a }
func (a *OptionArg[T]) Short(r rune) *OptionArg[T]       { a.b.short = r; return a }
func (a *OptionArg[T]) Env(name string) *OptionArg[T]    { a.b.env = name; return a }
func (a *OptionArg[T]) ValueName(n string) *OptionArg[T] { a.b.valueName = n; return a }
func (a *OptionArg[T]) Doc(summary string) *OptionArg[T] {
	a.b.doc.summary = summary
	return a
}
func (a *OptionArg[T]) Description(text string) *OptionArg[T] {
	a.b.doc.description = text
	return a
}
func (a *OptionArg[T]) Aliases(names ...string) *OptionArg[T] {
	a.b.aliases = append(a.b.aliases, names...)
	return a
}

// Default makes the option optional, taking v when it is left out.
func (a *OptionArg[T]) Default(v T) *OptionArg[T] {
	a.b.def = reflect.ValueOf(&v).Elem()
	return a
}

// ValueOptional lets the option be given bare (--mode), meaning v; it also
// takes v when left out entirely.
func (a *OptionArg[T]) ValueOptional(v T) *OptionArg[T] {
	a.b.valueOptional = true
	return a.Default(v)
}

// ValueIs refers to the option having the value v, for a constraint.
func (a *OptionArg[T]) ValueIs(v T) Ref    { return valueRef(a.b, v) }
func (a *OptionArg[T]) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *OptionArg[T]) toolRefs() refsDecl { return single(a) }

// ListArg is a slice field bound to an option given several times.
type ListArg[T any] struct {
	restrictable[*ListArg[T], T]
	b *argBinding
}

func newToolListArg[T any](b *argBinding) *ListArg[T] {
	a := &ListArg[T]{b: b}
	a.restrictable = restrictable[*ListArg[T], T]{rb: b, self: a}
	return a
}

func (a *ListArg[T]) Name(name string) *ListArg[T]   { a.b.name = name; return a }
func (a *ListArg[T]) Short(r rune) *ListArg[T]       { a.b.short = r; return a }
func (a *ListArg[T]) Env(name string) *ListArg[T]    { a.b.env = name; return a }
func (a *ListArg[T]) ValueName(n string) *ListArg[T] { a.b.valueName = n; return a }
func (a *ListArg[T]) Doc(summary string) *ListArg[T] { a.b.doc.summary = summary; return a }
func (a *ListArg[T]) Description(text string) *ListArg[T] {
	a.b.doc.description = text
	return a
}
func (a *ListArg[T]) Aliases(names ...string) *ListArg[T] {
	a.b.aliases = append(a.b.aliases, names...)
	return a
}

// Repeated takes one value per occurrence: --inc a --inc b. It is the default.
func (a *ListArg[T]) Repeated() *ListArg[T] {
	a.b.repetition = toolCommon.MakeRepetitionRepeated()
	return a
}

// Delimited takes the values in one occurrence: --inc=a,b.
func (a *ListArg[T]) Delimited(sep rune) *ListArg[T] {
	a.b.repetition = toolCommon.MakeRepetitionDelimited(sep)
	return a
}

// Either accepts both forms.
func (a *ListArg[T]) Either(sep rune) *ListArg[T] {
	a.b.repetition = toolCommon.MakeRepetitionEither(sep)
	return a
}

// Default is the list taken when the option is not given.
func (a *ListArg[T]) Default(v []T) *ListArg[T] {
	a.b.def = reflect.ValueOf(&v).Elem()
	return a
}

// ValueIs refers to one of the values being v, for a constraint.
func (a *ListArg[T]) ValueIs(v T) Ref    { return valueRef(a.b, v) }
func (a *ListArg[T]) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *ListArg[T]) toolRefs() refsDecl { return single(a) }

// MapArg is a map field bound to a key-value option.
type MapArg[K comparable, V any] struct{ b *argBinding }

func (a *MapArg[K, V]) Name(name string) *MapArg[K, V]   { a.b.name = name; return a }
func (a *MapArg[K, V]) Short(r rune) *MapArg[K, V]       { a.b.short = r; return a }
func (a *MapArg[K, V]) Env(name string) *MapArg[K, V]    { a.b.env = name; return a }
func (a *MapArg[K, V]) ValueName(n string) *MapArg[K, V] { a.b.valueName = n; return a }
func (a *MapArg[K, V]) Doc(summary string) *MapArg[K, V] {
	a.b.doc.summary = summary
	return a
}
func (a *MapArg[K, V]) Description(text string) *MapArg[K, V] {
	a.b.doc.description = text
	return a
}
func (a *MapArg[K, V]) Aliases(names ...string) *MapArg[K, V] {
	a.b.aliases = append(a.b.aliases, names...)
	return a
}

// Repeated takes one pair per occurrence: -c a=1 -c b=2. It is the default.
func (a *MapArg[K, V]) Repeated() *MapArg[K, V] {
	a.b.repetition = toolCommon.MakeRepetitionRepeated()
	return a
}

// Delimited takes the pairs in one occurrence: -c a=1,b=2.
func (a *MapArg[K, V]) Delimited(sep rune) *MapArg[K, V] {
	a.b.repetition = toolCommon.MakeRepetitionDelimited(sep)
	return a
}

// Either accepts both forms.
func (a *MapArg[K, V]) Either(sep rune) *MapArg[K, V] {
	a.b.repetition = toolCommon.MakeRepetitionEither(sep)
	return a
}

// Default is the map taken when the option is not given.
func (a *MapArg[K, V]) Default(v map[K]V) *MapArg[K, V] {
	a.b.def = reflect.ValueOf(&v).Elem()
	return a
}

// LastKeyWins keeps the last value of a repeated key; by default a repeated
// key is a usage error.
func (a *MapArg[K, V]) LastKeyWins() *MapArg[K, V] { a.b.lastKeyWins = true; return a }

// ValueIs refers to one of the map's values being v, for a constraint.
func (a *MapArg[K, V]) ValueIs(v V) Ref    { return valueRef(a.b, v) }
func (a *MapArg[K, V]) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *MapArg[K, V]) toolRefs() refsDecl { return single(a) }

// FlagArg is a bool field bound to a switch.
type FlagArg struct{ b *argBinding }

func (a *FlagArg) Name(name string) *FlagArg   { a.b.name = name; return a }
func (a *FlagArg) Short(r rune) *FlagArg       { a.b.short = r; return a }
func (a *FlagArg) Env(name string) *FlagArg    { a.b.env = name; return a }
func (a *FlagArg) Doc(summary string) *FlagArg { a.b.doc.summary = summary; return a }
func (a *FlagArg) Description(text string) *FlagArg {
	a.b.doc.description = text
	return a
}
func (a *FlagArg) Aliases(names ...string) *FlagArg {
	a.b.aliases = append(a.b.aliases, names...)
	return a
}

// Negatable also accepts --no-<name>, setting the flag to false.
func (a *FlagArg) Negatable() *FlagArg { a.b.negatable = true; return a }

// Default sets the flag's value when it is not given.
func (a *FlagArg) Default(v bool) *FlagArg { a.b.flagDefault = v; return a }

func (a *FlagArg) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *FlagArg) toolRefs() refsDecl { return single(a) }

// CountFlagArg is a uint32 field bound to a counted switch.
type CountFlagArg struct{ b *argBinding }

func (a *CountFlagArg) Name(name string) *CountFlagArg   { a.b.name = name; return a }
func (a *CountFlagArg) Short(r rune) *CountFlagArg       { a.b.short = r; return a }
func (a *CountFlagArg) Env(name string) *CountFlagArg    { a.b.env = name; return a }
func (a *CountFlagArg) Doc(summary string) *CountFlagArg { a.b.doc.summary = summary; return a }
func (a *CountFlagArg) Description(text string) *CountFlagArg {
	a.b.doc.description = text
	return a
}
func (a *CountFlagArg) Aliases(names ...string) *CountFlagArg {
	a.b.aliases = append(a.b.aliases, names...)
	return a
}

// Max caps the count.
func (a *CountFlagArg) Max(n uint32) *CountFlagArg { a.b.max = &n; return a }

func (a *CountFlagArg) toolRef() refDecl   { return refDecl{b: a.b} }
func (a *CountFlagArg) toolRefs() refsDecl { return single(a) }

// StdinArg is the io.Reader field a command reads standard input from.
type StdinArg struct{ b *stdinBinding }

func (a *StdinArg) Doc(summary string) *StdinArg { a.b.doc.summary = summary; return a }
func (a *StdinArg) Description(text string) *StdinArg {
	a.b.doc.description = text
	return a
}

// Mime lists the media types the command accepts.
func (a *StdinArg) Mime(mime ...string) *StdinArg {
	a.b.mime = append(a.b.mime, mime...)
	return a
}

// Optional lets the caller leave the reader nil.
func (a *StdinArg) Optional() *StdinArg { a.b.optional = true; return a }

// Constraints.

// Ref refers to an argument inside a constraint: a binding refers to the
// argument being given, and ValueIs to it having a particular value.
type Ref interface {
	Refs
	toolRef() refDecl
}

// Refs is a quantified group of references. A single ToolRef is a group
// that holds when it does; build larger ones with AllOf and AnyOf.
type Refs interface{ toolRefs() refsDecl }

type refDecl struct {
	b *argBinding
	// path refers to a field by pointer instead, resolved against the whole
	// argument struct (globals included) when the command is built.
	path  []int
	value reflect.Value
}

type refsDecl struct {
	quant toolCommon.Quantifier
	refs  []refDecl
}

type valueRefDecl struct{ r refDecl }

func (v valueRefDecl) toolRef() refDecl { return v.r }
func (v valueRefDecl) toolRefs() refsDecl {
	return refsDecl{quant: toolCommon.QuantifierAll, refs: []refDecl{v.r}}
}

type refSet struct{ d refsDecl }

func (s refSet) toolRefs() refsDecl { return s.d }

func single(r Ref) refsDecl {
	return refsDecl{quant: toolCommon.QuantifierAll, refs: []refDecl{r.toolRef()}}
}

func valueRef[T any](b *argBinding, v T) Ref {
	return valueRefDecl{r: refDecl{b: b, value: reflect.ValueOf(&v).Elem()}}
}

func refsOf(refs []Ref) []refDecl {
	out := make([]refDecl, 0, len(refs))
	for _, r := range refs {
		out = append(out, r.toolRef())
	}
	return out
}

// Present refers to a field of the argument struct being given, by pointer. It
// reaches inherited globals too, which have no binding in the command's spec.
func (s *CommandSpec) Present(p any) Ref {
	path, _, err := s.state.target.resolve(p)
	if err != nil {
		s.state.fail("Present: %v", err)
	}
	return valueRefDecl{r: refDecl{path: path}}
}

// ValueIs refers to a field of the argument struct having the value v, by
// pointer, like Present.
func (s *CommandSpec) ValueIs[T any](p *T, v T) Ref {
	path, _, err := s.state.target.resolve(p)
	if err != nil {
		s.state.fail("ValueIs: %v", err)
	}
	return valueRefDecl{r: refDecl{path: path, value: reflect.ValueOf(&v).Elem()}}
}

// AllOf holds when every reference holds.
func (s *CommandSpec) AllOf(refs ...Ref) Refs {
	return refSet{refsDecl{quant: toolCommon.QuantifierAll, refs: refsOf(refs)}}
}

// AnyOf holds when at least one reference holds.
func (s *CommandSpec) AnyOf(refs ...Ref) Refs {
	return refSet{refsDecl{quant: toolCommon.QuantifierAny, refs: refsOf(refs)}}
}

type constraintKind uint8

const (
	constraintRequiresAll constraintKind = iota
	constraintAllOrNone
	constraintRequiresAny
	constraintMutexGroups
	constraintImplies
	constraintForbids
)

type constraintDecl struct {
	kind   constraintKind
	refs   []refDecl
	groups [][]refDecl
	lhs    refsDecl
	rhs    refsDecl
}

// RequiresAll holds only when every reference holds.
func (s *CommandSpec) RequiresAll(refs ...Ref) {
	s.constraints = append(s.constraints, constraintDecl{kind: constraintRequiresAll, refs: refsOf(refs)})
}

// RequiresAny holds when at least one reference holds.
func (s *CommandSpec) RequiresAny(refs ...Ref) {
	s.constraints = append(s.constraints, constraintDecl{kind: constraintRequiresAny, refs: refsOf(refs)})
}

// AllOrNone holds when either every reference holds or none does.
func (s *CommandSpec) AllOrNone(refs ...Ref) {
	s.constraints = append(s.constraints, constraintDecl{kind: constraintAllOrNone, refs: refsOf(refs)})
}

// Mutex allows at most one of the references to hold.
func (s *CommandSpec) Mutex(refs ...Ref) {
	groups := make([][]refDecl, 0, len(refs))
	for _, r := range refs {
		groups = append(groups, []refDecl{r.toolRef()})
	}
	s.constraints = append(s.constraints, constraintDecl{kind: constraintMutexGroups, groups: groups})
}

// MutexGroups allows at most one group to hold in full.
func (s *CommandSpec) MutexGroups(groups ...Refs) {
	out := make([][]refDecl, 0, len(groups))
	for _, g := range groups {
		out = append(out, g.toolRefs().refs)
	}
	s.constraints = append(s.constraints, constraintDecl{kind: constraintMutexGroups, groups: out})
}

// Implies requires rhs to hold whenever lhs does.
func (s *CommandSpec) Implies(lhs, rhs Refs) {
	s.constraints = append(s.constraints, constraintDecl{kind: constraintImplies, lhs: lhs.toolRefs(), rhs: rhs.toolRefs()})
}

// Forbids rejects any of rhs holding whenever lhs does.
func (s *CommandSpec) Forbids(lhs Refs, rhs ...Ref) {
	s.constraints = append(s.constraints, constraintDecl{
		kind: constraintForbids, lhs: lhs.toolRefs(),
		rhs: refsDecl{quant: toolCommon.QuantifierAny, refs: refsOf(rhs)},
	})
}

// kebab turns a Go field name into a command-line name: GitDir is git-dir,
// MaxCount is max-count and URLPath is url-path.
func kebab(name string) string {
	runes := []rune(name)
	var b strings.Builder
	for i, r := range runes {
		if unicode.IsUpper(r) {
			prevLower := i > 0 && (unicode.IsLower(runes[i-1]) || unicode.IsDigit(runes[i-1]))
			nextLower := i+1 < len(runes) && unicode.IsLower(runes[i+1])
			prevUpper := i > 0 && unicode.IsUpper(runes[i-1])
			if i > 0 && (prevLower || (prevUpper && nextLower)) {
				b.WriteByte('-')
			}
			b.WriteRune(unicode.ToLower(r))
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

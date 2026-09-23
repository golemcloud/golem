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

package golem

import (
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Tool definitions.
//
// A tool is a callable unit declared from one piece of metadata: a command tree
// whose bodies take arguments (see [Positional], [Opt] and [Flag]) and return a
// result. The tool's identity is its root command name.
//
//	var Greeter = golem.DefineTool("greeter", golem.ToolSpec{
//	    Version: "1.0.0",
//	    Summary: "Greets people",
//	})
//
//	var Greet = golem.Command[GreetArgs, string](Greeter, "greet", GreetArgs{...})
//
//	var _ = golem.HandleCommand(Greet, func(ctx *golem.ToolContext, in GreetArgs) string {
//	    return "hi " + in.Name.Get()
//	})

// ToolSpec describes a tool as a whole.
type ToolSpec struct {
	// Version is the tool's own version, reported in its metadata.
	Version string
	// Summary is the one-line description shown in help output.
	Summary string
	// Description is the longer prose shown for the tool itself.
	Description string
	// Aliases are additional names the root command answers to.
	Aliases []string
}

// CommandOpt customises one command.
type CommandOpt func(*commandOpts)

type commandOpts struct {
	summary     string
	description string
	aliases     []string
	stdin       *StreamSpec
	stdout      *StreamSpec
}

// Summary sets a command's one-line description.
func Summary(s string) CommandOpt {
	return func(o *commandOpts) { o.summary = s }
}

// Description sets a command's longer prose description.
func Description(s string) CommandOpt {
	return func(o *commandOpts) { o.description = s }
}

// Aliases adds alternative names for a command.
func Aliases(names ...string) CommandOpt {
	return func(o *commandOpts) { o.aliases = append(o.aliases, names...) }
}

// StreamSpec describes a command's standard input or output stream.
type StreamSpec struct {
	Doc string
	// Mime lists the accepted or produced media types. Empty means unrestricted.
	Mime []string
	// Required rejects an invocation that does not supply the stream.
	Required bool
}

// Stdin declares that the command reads standard input.
func Stdin(spec StreamSpec) CommandOpt {
	return func(o *commandOpts) { o.stdin = &spec }
}

// Stdout declares that the command writes standard output.
func Stdout(spec StreamSpec) CommandOpt {
	return func(o *commandOpts) { o.stdout = &spec }
}

// ToolDefinition is a registered tool, returned by [DefineTool].
type ToolDefinition struct {
	name string
	spec ToolSpec
}

// Name returns the tool's name, which is also its root command name.
func (t *ToolDefinition) Name() string { return t.name }

// commandEntry is one declared command body plus the handler bound to it.
type commandEntry struct {
	// path is the command path from the root; empty addresses the root body.
	path []string
	name string
	opts commandOpts
	// argsType is the Go struct describing the body's arguments, and proto the
	// declaration-time prototype the metadata is read from.
	argsType reflect.Type
	proto    reflect.Value
	outType  reflect.Type
	// invoke is installed by HandleCommand; a command without one is a
	// definition error.
	invoke func(*ToolContext, reflect.Value) reflect.Value
}

// toolEntry is a registered tool: its spec and its commands in declaration order.
type toolEntry struct {
	def      *ToolDefinition
	commands []*commandEntry
	byPath   map[string]*commandEntry
}

// DefineTool registers a tool. Call it from a package-level var so registration
// happens before the component is invoked.
func DefineTool(name string, spec ToolSpec) *ToolDefinition {
	return defineToolInto(toolDefs, defs, name, spec)
}

// defineToolInto is the instance-scoped implementation behind DefineTool.
func defineToolInto(r *toolRegistry, d *definitions, name string, spec ToolSpec) *ToolDefinition {
	t := &ToolDefinition{name: name, spec: spec}
	if name == "" {
		d.recordErr("", "", "DefineTool requires a name")
		return t
	}
	if _, dup := r.byName[name]; dup {
		d.recordErr("", "", "tool already defined: %s", name)
		return t
	}
	r.order = append(r.order, name)
	r.byName[name] = &toolEntry{def: t, byPath: map[string]*commandEntry{}}
	return t
}

// CommandDef identifies one command body, and carries the argument and result
// types so [HandleCommand] can check the handler against them.
type CommandDef[Args any, Out any] struct {
	tool string
	path []string
}

// Path returns the command's path from the root; empty addresses the root body.
func (c CommandDef[Args, Out]) Path() []string { return c.path }

// Body declares the root command's own body: what the tool does when invoked
// with no subcommand.
func Body[Args any, Out any](t *ToolDefinition, proto Args, opts ...CommandOpt) CommandDef[Args, Out] {
	return declareCommand[Args, Out](toolDefs, defs, t, nil, "", proto, opts)
}

// Command declares a subcommand of the tool's root.
func Command[Args any, Out any](t *ToolDefinition, name string, proto Args, opts ...CommandOpt) CommandDef[Args, Out] {
	return declareCommand[Args, Out](toolDefs, defs, t, []string{name}, name, proto, opts)
}

func declareCommand[Args any, Out any](
	r *toolRegistry, d *definitions, t *ToolDefinition,
	path []string, name string, proto Args, opts []CommandOpt,
) CommandDef[Args, Out] {
	def := CommandDef[Args, Out]{tool: t.name, path: path}
	e := r.byName[t.name]
	if e == nil {
		d.recordErr("", "", "command %q declared on unregistered tool %q", name, t.name)
		return def
	}
	key := pathKey(path)
	if _, dup := e.byPath[key]; dup {
		d.recordErr("", "", "tool %s: command already declared: %s", t.name, commandLabel(path))
		return def
	}
	var co commandOpts
	for _, o := range opts {
		o(&co)
	}
	ce := &commandEntry{
		path:     path,
		name:     name,
		opts:     co,
		argsType: reflect.TypeFor[Args](),
		proto:    reflect.ValueOf(proto),
		outType:  reflect.TypeFor[Out](),
	}
	e.commands = append(e.commands, ce)
	e.byPath[key] = ce
	return def
}

// HandleCommand binds the implementation of a declared command. Call it from a
// package-level var so the binding happens before the component is invoked.
func HandleCommand[Args any, Out any](c CommandDef[Args, Out], h func(*ToolContext, Args) Out) Registered {
	return handleCommandInto(toolDefs, defs, c, h)
}

func handleCommandInto[Args any, Out any](
	r *toolRegistry, d *definitions, c CommandDef[Args, Out], h func(*ToolContext, Args) Out,
) Registered {
	e := r.byName[c.tool]
	if e == nil {
		d.recordErr("", "", "handler declared for a command of unregistered tool %q", c.tool)
		return Registered{}
	}
	ce := e.byPath[pathKey(c.path)]
	if ce == nil {
		d.recordErr("", "", "tool %s: handler declared for an undeclared command %s", c.tool, commandLabel(c.path))
		return Registered{}
	}
	if ce.invoke != nil {
		d.recordErr("", "", "tool %s: command %s already has a handler", c.tool, commandLabel(c.path))
		return Registered{}
	}
	ce.invoke = func(ctx *ToolContext, in reflect.Value) reflect.Value {
		return reflect.ValueOf(h(ctx, in.Interface().(Args)))
	}
	return Registered{}
}

// pathKey makes a command path usable as a map key; the separator cannot occur
// in a command name, which the WIT restricts to lower-case words and dashes.
func pathKey(path []string) string {
	key := ""
	for _, seg := range path {
		key += seg + "\x00"
	}
	return key
}

func commandLabel(path []string) string {
	if len(path) == 0 {
		return "<root>"
	}
	label := path[0]
	for _, seg := range path[1:] {
		label += " " + seg
	}
	return label
}

// toolArgFields describes one command's arguments, read off the prototype.
type toolArgField struct {
	name  string
	index int
	kind  toolArgKind
	elem  reflect.Type
	meta  toolArgMeta
	codec *codec
}

// argFields reads a command's argument fields from its prototype. Every
// exported field must be one of the argument markers; anything else is a
// definition error, because the deriver would otherwise have no way to say
// whether it is a positional, an option or a flag.
func (d *definitions) argFields(toolName string, ce *commandEntry) ([]toolArgField, bool) {
	t := ce.argsType
	if t.Kind() != reflect.Struct {
		d.recordErr("", "", "tool %s: command %s takes %s, but command arguments must be a struct",
			toolName, commandLabel(ce.path), t)
		return nil, false
	}
	var out []toolArgField
	ok := true
	for i := range t.NumField() {
		f := t.Field(i)
		if f.PkgPath != "" { // unexported
			continue
		}
		arg, isArg := ce.proto.Field(i).Interface().(toolArg)
		if !isArg {
			d.recordErr("", "", "tool %s: command %s field %s is %s; command arguments must be golem.Positional, golem.Opt or golem.Flag",
				toolName, commandLabel(ce.path), f.Name, f.Type)
			ok = false
			continue
		}
		out = append(out, toolArgField{
			name:  lowerFirst(f.Name),
			index: i,
			kind:  arg.toolArgKind(),
			elem:  arg.toolArgElem(),
			meta:  arg.toolArgMeta(),
			codec: d.compile(arg.toolArgElem()),
		})
	}
	return out, ok
}

func docOf(summary, description string) toolCommon.Doc {
	return toolCommon.Doc{Summary: summary, Description: description}
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

// defaultTree encodes a declared default against its own type, ready to be
// interpreted against the matching node in the tool's schema.
func defaultTree(f toolArgField) witTypes.Option[types.SchemaValueTree] {
	if !f.meta.def.IsValid() {
		return witTypes.None[types.SchemaValueTree]()
	}
	return witTypes.Some(encodeWith(f.codec, f.meta.def))
}

// streamSpec converts a declared stream to its WIT form.
func streamSpec(s *StreamSpec) witTypes.Option[toolCommon.StreamSpec] {
	if s == nil {
		return witTypes.None[toolCommon.StreamSpec]()
	}
	return witTypes.Some(toolCommon.StreamSpec{
		Doc:      docOf(s.Doc, ""),
		Mime:     append([]string(nil), s.Mime...),
		Required: s.Required,
	})
}

// buildTool derives the metadata the host discovers for one tool. The second
// result reports whether derivation succeeded; failures are recorded on d.
func (d *definitions) buildTool(e *toolEntry) (toolCommon.Tool, bool) {
	g := graphBuilder{d: d}
	ok := true

	// The root is always node 0 of the command tree, and subcommands are
	// appended, so the root's child indices are known once they are all built.
	root := toolCommon.CommandNode{
		Name:    e.def.name,
		Aliases: append([]string(nil), e.def.spec.Aliases...),
		Doc:     docOf(e.def.spec.Summary, e.def.spec.Description),
		Body:    witTypes.None[toolCommon.CommandBody](),
	}
	nodes := []toolCommon.CommandNode{root}

	for _, ce := range e.commands {
		if ce.invoke == nil {
			d.recordErr("", "", "tool %s: command %s has no handler; call golem.HandleCommand",
				e.def.name, commandLabel(ce.path))
			ok = false
		}
		fields, fieldsOK := d.argFields(e.def.name, ce)
		if !fieldsOK {
			ok = false
		}
		body := d.buildCommandBody(&g, ce, fields)
		if len(ce.path) == 0 {
			nodes[0].Body = witTypes.Some(body)
			continue
		}
		nodes = append(nodes, toolCommon.CommandNode{
			Name:    ce.name,
			Aliases: append([]string(nil), ce.opts.aliases...),
			Doc:     docOf(ce.opts.summary, ce.opts.description),
			Body:    witTypes.Some(body),
		})
		nodes[0].Subcommands = append(nodes[0].Subcommands, int32(len(nodes)-1))
	}

	for typ, why := range g.invalids {
		d.recordErr("", "", "tool %s references %s, which cannot be represented: %s", e.def.name, typ, why)
		ok = false
	}

	return toolCommon.Tool{
		Version:  e.def.spec.Version,
		Commands: toolCommon.CommandTree{Nodes: nodes},
		Schema:   g.build(),
	}, ok
}

func (d *definitions) buildCommandBody(g *graphBuilder, ce *commandEntry, fields []toolArgField) toolCommon.CommandBody {
	var positionals []toolCommon.Positional
	var options []toolCommon.OptionSpec
	var flags []toolCommon.FlagSpec

	for _, f := range fields {
		switch f.kind {
		case argPositional:
			positionals = append(positionals, toolCommon.Positional{
				Name:         f.name,
				Doc:          docOf(f.meta.doc, ""),
				ValueName:    someIfSet(f.meta.valueName),
				Type:         g.node(f.codec),
				Default:      defaultTree(f),
				Required:     f.meta.required,
				AcceptsStdio: f.meta.acceptsStdin,
			})
		case argOption:
			options = append(options, toolCommon.OptionSpec{
				Long:      f.name,
				Short:     shortOf(f.meta.short),
				Aliases:   append([]string(nil), f.meta.aliases...),
				Doc:       docOf(f.meta.doc, ""),
				ValueName: someIfSet(f.meta.valueName),
				Shape:     d.optionShape(g, ce, f),
				Default:   defaultTree(f),
				Required:  f.meta.required,
				EnvVar:    someIfSet(f.meta.envVar),
			})
		case argFlag:
			flags = append(flags, toolCommon.FlagSpec{
				Long:    f.name,
				Short:   shortOf(f.meta.short),
				Aliases: append([]string(nil), f.meta.aliases...),
				Doc:     docOf(f.meta.doc, ""),
				Shape:   toolCommon.MakeFlagShapeBoolFlag(toolCommon.BoolFlagShape{}),
				EnvVar:  someIfSet(f.meta.envVar),
			})
		}
	}

	result := witTypes.None[toolCommon.ResultSpec]()
	if ce.outType != reflect.TypeFor[Unit]() {
		result = witTypes.Some(toolCommon.ResultSpec{
			Type: g.node(d.compile(ce.outType)),
			Doc:  docOf("", ""),
		})
	}

	return toolCommon.CommandBody{
		Positionals: toolCommon.Positionals{
			Fixed: positionals,
			Tail:  witTypes.None[toolCommon.TailPositional](),
		},
		Options:     options,
		Flags:       flags,
		Stdin:       streamSpec(ce.opts.stdin),
		Stdout:      streamSpec(ce.opts.stdout),
		Result:      result,
		Annotations: witTypes.None[toolCommon.CommandAnnotations](),
	}
}

// decodeToolArgs fills an args struct from the invocation input: a record with
// one field per declared argument, in declaration order. Each value is decoded
// against the argument's payload type and written through the marker's setter,
// so the handler reads it with Get.
func decodeToolArgs(tree types.SchemaValueTree, fields []toolArgField, dst reflect.Value) error {
	d := decoder{nodes: tree.ValueNodes}
	if len(d.nodes) == 0 {
		if len(fields) == 0 {
			return nil
		}
		return fmt.Errorf("empty value tree but %d argument(s) expected", len(fields))
	}
	root, err := d.node(tree.Root)
	if err != nil {
		return err
	}
	if root.Tag() != types.SchemaValueNodeRecordValue {
		if len(fields) == 0 {
			return nil
		}
		return fmt.Errorf("expected a record at the root of the argument list")
	}
	idxs := root.RecordValue()
	if len(idxs) < len(fields) {
		return fmt.Errorf("argument list has %d value(s), want %d", len(idxs), len(fields))
	}
	for i, f := range fields {
		slot := reflect.New(f.elem).Elem()
		if err := f.codec.decode(&d, slot, idxs[i]); err != nil {
			return fmt.Errorf("argument %q: %w", f.name, err)
		}
		setter, ok := dst.Field(f.index).Addr().Interface().(toolArgSetter)
		if !ok {
			return fmt.Errorf("argument %q: %s is not a tool argument marker", f.name, f.elem)
		}
		setter.toolArgSet(slot)
	}
	return nil
}

// invokeCommand runs one command: resolve it, decode the arguments, call the
// handler, and package the result as a self-contained typed value.
func (d *definitions) invokeCommand(
	e *toolEntry, commandPath []string, input types.TypedSchemaValue,
	stdin *ToolStdin, stdout *ToolStdout,
) witTypes.Result[toolCommon.InvocationResult, types.ToolError] {
	ce, ok := e.byPath[pathKey(commandPath)]
	if !ok {
		return witTypes.Err[toolCommon.InvocationResult](
			types.MakeToolErrorInvalidCommandPath(append([]string(nil), commandPath...)))
	}
	if ce.invoke == nil {
		return witTypes.Err[toolCommon.InvocationResult](toolDefinitionError(d))
	}
	fields, fieldsOK := d.argFields(e.def.name, ce)
	if !fieldsOK {
		return witTypes.Err[toolCommon.InvocationResult](toolDefinitionError(d))
	}

	args := reflect.New(ce.argsType).Elem()
	// The prototype carries the declared metadata; starting from a copy of it
	// keeps any declared default visible to a handler whose argument the caller
	// omitted.
	args.Set(ce.proto)
	if err := decodeToolArgs(input.Value, fields, args); err != nil {
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidInput(err.Error()))
	}

	ctx := &ToolContext{tool: e.def.name, path: commandPath, stdin: stdin, stdout: stdout}
	out, err := runCommandHandler(ce, ctx, args)
	if err != nil {
		return witTypes.Err[toolCommon.InvocationResult](types.MakeToolErrorInvalidResult(err.Error()))
	}

	if ce.outType == reflect.TypeFor[Unit]() {
		return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
			Result: witTypes.None[types.TypedSchemaValue](),
			Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
		})
	}

	outCodec := d.compile(ce.outType)
	g := graphBuilder{d: d}
	root := g.node(outCodec)
	graph := g.build()
	graph.Root = root
	return witTypes.Ok[toolCommon.InvocationResult, types.ToolError](toolCommon.InvocationResult{
		Result: witTypes.Some(types.TypedSchemaValue{
			Graph: graph,
			Value: encodeWith(outCodec, out),
		}),
		Stdout: witTypes.None[*witTypes.StreamReader[uint8]](),
	})
}

// optionShape picks the wire shape an option's declaration asks for. The four
// shapes are mutually exclusive, so a contradictory declaration is a definition
// error rather than a silently-chosen winner.
func (d *definitions) optionShape(g *graphBuilder, ce *commandEntry, f toolArgField) toolCommon.OptionShape {
	meta := f.meta
	switch {
	case meta.repeatable.set && meta.valueOptional:
		d.recordErr("", "", "command %s option %q is both ValueOptional and Repeatable; an option has one shape",
			commandLabel(ce.path), f.name)
		return toolCommon.MakeOptionShapeScalar(g.node(f.codec))

	case meta.repeatable.set:
		switch f.elem.Kind() {
		case reflect.Map:
			return toolCommon.MakeOptionShapeRepeatableMap(toolCommon.RepeatableMapShape{
				Repetition:         meta.repeatable.rep,
				MapType:            g.node(f.codec),
				DuplicateKeyPolicy: uint8(meta.duplicateKeys),
			})
		case reflect.Slice:
			return toolCommon.MakeOptionShapeRepeatableList(toolCommon.RepeatableListShape{
				Repetition: meta.repeatable.rep,
				ItemType:   g.node(d.compile(f.elem.Elem())),
			})
		default:
			d.recordErr("", "", "command %s option %q is Repeatable but collects into %s; use a slice or a map",
				commandLabel(ce.path), f.name, f.elem)
			return toolCommon.MakeOptionShapeScalar(g.node(f.codec))
		}

	case meta.valueOptional:
		if !meta.def.IsValid() {
			// Bare presence means the declared default, so without one there is
			// nothing for it to mean.
			d.recordErr("", "", "command %s option %q is ValueOptional but declares no Default",
				commandLabel(ce.path), f.name)
		}
		return toolCommon.MakeOptionShapeOptionalScalar(g.node(f.codec))

	default:
		return toolCommon.MakeOptionShapeScalar(g.node(f.codec))
	}
}

// runCommandHandler calls the handler and selects the output stream's terminal
// for it: finished when the handler returns, failed when it panics. The wire
// accepts exactly one terminal and treats a dropped writer as abandoned, so
// choosing one here is what keeps a panicking handler from silently looking
// like an abandoned transfer.
func runCommandHandler(ce *commandEntry, ctx *ToolContext, args reflect.Value) (out reflect.Value, err error) {
	finished := false
	defer func() {
		if r := recover(); r != nil {
			_ = ctx.stdout.Fail(StreamFailed(panicMessage(r)))
			panic(r)
		}
		if !finished {
			return
		}
		if ferr := ctx.stdout.finish(); ferr != nil && err == nil {
			err = ferr
		}
	}()
	out = ce.invoke(ctx, args)
	finished = true
	return out, nil
}

// panicMessage renders a recovered panic for the stream failure reason.
func panicMessage(r any) string {
	if e, ok := r.(error); ok {
		return e.Error()
	}
	return fmt.Sprintf("%v", r)
}

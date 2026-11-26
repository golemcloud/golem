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

package reflection

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"reflect"
	"strconv"
	"strings"
	"unicode"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Arguments is a command line read against a tool's metadata: either the help
// text it asked for, or the command it selects and that command's input.
type Arguments struct {
	// Help is the selected command's help text when the command line asked
	// for help. Path and Input are then empty.
	Help string
	// Path is the selected command's canonical path, without the tool's name.
	Path []string
	// Input holds the command's named arguments as canonical JSON, ready for
	// [ToolClient.Start].
	Input map[string]any
}

// ParseArgs reads a command line the way every Golem caller does: the Golem
// CLI, the bash tool and the other SDKs read it with the same rules for
// options, flags, subcommands, aliases, positionals, defaults, help and
// constraints. args excludes the tool's name.
//
// An error is a usage diagnostic for the person who wrote the command line.
func (r Tool) ParseArgs(args []string) (Arguments, error) {
	return r.ParseArgsWithVariables(args, nil)
}

// ParseArgsWithVariables reads a command line like [Tool.ParseArgs]. An option
// or flag that the command line leaves out and that declares a variable takes
// that variable's value from variables, as if it was written once. The command
// line wins over a variable, and a variable wins over a default. Only variables
// is read, never the process environment.
func (r Tool) ParseArgsWithVariables(args []string, variables map[string]string) (Arguments, error) {
	if len(r.wit.Commands.Nodes) == 0 {
		return Arguments{}, errors.New("tool metadata has no root command")
	}
	p := argParser{
		tool:           r,
		cmd:            r.Root(),
		supplied:       map[string][]string{},
		flags:          map[string]*flagState{},
		optionsEnabled: true,
	}
	for p.i = 0; p.i < len(args); p.i++ {
		help, err := p.step(args)
		if err != nil {
			return Arguments{}, err
		}
		if help {
			return Arguments{Help: p.renderHelp()}, nil
		}
	}
	return p.finish(variables)
}

type flagState struct {
	count   uint32
	boolean *bool
}

// argSurface is one named or positional argument of a command.
type argSurface struct {
	option     *toolCommon.OptionSpec
	flag       *toolCommon.FlagSpec
	positional int
	tail       bool
	// inBody is set for an option or flag that the command declares itself,
	// not one it inherits.
	inBody bool
}

type argParser struct {
	tool        Tool
	cmd         Command
	i           int
	supplied    map[string][]string
	flags       map[string]*flagState
	positionals []string
	tail        []string

	optionsEnabled bool
	tailStarted    bool
	bodyStarted    bool
}

func (p *argParser) node() toolCommon.CommandNode { return p.cmd.node() }

func (p *argParser) body() *toolCommon.CommandBody {
	n := p.node()
	if n.Body.IsNone() {
		return nil
	}
	b := n.Body.Some()
	return &b
}

// named lists the options and flags the command accepts: inherited globals
// root first, options before flags per node, then the command's own.
func (p *argParser) named() []argSurface {
	return namedSurfaces(p.cmd)
}

func namedSurfaces(c Command) []argSurface {
	var out []argSurface
	for _, idx := range c.chain {
		g := c.tree.Nodes[idx].Globals
		for i := range g.Options {
			out = append(out, argSurface{option: &g.Options[i]})
		}
		for i := range g.Flags {
			out = append(out, argSurface{flag: &g.Flags[i]})
		}
	}
	if b := c.node().Body; b.IsSome() {
		body := b.Some()
		for i := range body.Options {
			out = append(out, argSurface{option: &body.Options[i], inBody: true})
		}
		for i := range body.Flags {
			out = append(out, argSurface{flag: &body.Flags[i], inBody: true})
		}
	}
	return out
}

// surfaces lists the command's arguments in the order of its canonical input
// record, which is the order of [Command.canonicalFields].
func surfaces(c Command) []argSurface {
	var out []argSurface
	for _, idx := range c.chain {
		g := c.tree.Nodes[idx].Globals
		for i := range g.Options {
			out = append(out, argSurface{option: &g.Options[i]})
		}
		for i := range g.Flags {
			out = append(out, argSurface{flag: &g.Flags[i]})
		}
	}
	body := c.node().Body.Some()
	for i := range body.Positionals.Fixed {
		out = append(out, argSurface{positional: i})
	}
	if body.Positionals.Tail.IsSome() {
		out = append(out, argSurface{tail: true})
	}
	for i := range body.Options {
		out = append(out, argSurface{option: &body.Options[i], inBody: true})
	}
	for i := range body.Flags {
		out = append(out, argSurface{flag: &body.Flags[i], inBody: true})
	}
	return out
}

func hasName(long string, aliases []string, name string) bool {
	if long == name {
		return true
	}
	for _, a := range aliases {
		if a == name {
			return true
		}
	}
	return false
}

func findShort(named []argSurface, short rune) (argSurface, bool) {
	for _, s := range named {
		switch {
		case s.option != nil && s.option.Short.IsSome() && s.option.Short.Some() == short:
			return s, true
		case s.flag != nil && s.flag.Short.IsSome() && s.flag.Short.Some() == short:
			return s, true
		}
	}
	return argSurface{}, false
}

func findLongFlag(named []argSurface, name string) (*toolCommon.FlagSpec, bool, bool) {
	for _, s := range named {
		if s.flag != nil && hasName(s.flag.Long, s.flag.Aliases, name) {
			return s.flag, false, true
		}
	}
	for _, s := range named {
		if s.flag != nil && s.flag.Shape.Tag() == toolCommon.FlagShapeBoolFlag &&
			s.flag.Shape.BoolFlag().Negatable && name == "no-"+s.flag.Long {
			return s.flag, true, true
		}
	}
	return nil, false, false
}

// implicitTail reports whether the command's tail can start without its
// separator: a verbatim tail after `--` is its only positional surface and it
// has no subcommands, so the first token that is not one of its options or
// flags starts the tail.
func implicitTail(c Command) bool {
	n := c.node()
	if len(n.Subcommands) != 0 || n.Body.IsNone() {
		return false
	}
	pos := n.Body.Some().Positionals
	if len(pos.Fixed) != 0 || pos.Tail.IsNone() {
		return false
	}
	t := pos.Tail.Some()
	return t.Verbatim && t.Separator.IsSome() && t.Separator.Some() == "--"
}

// declaredShortCluster reports whether each character of a short-option
// cluster is the command's, read as the parser reads it: declared flags, up to
// a declared option that takes the rest as its value, or a help request.
func declaredShortCluster(c Command, named []argSurface, cluster string) bool {
	help := helpAvailable(c)
	for _, short := range cluster {
		if help && short == 'h' {
			return true
		}
		s, found := findShort(named, short)
		if !found {
			return false
		}
		if s.flag == nil {
			return true
		}
	}
	return cluster != ""
}

func helpAvailable(c Command) bool {
	if b := c.node().Body; b.IsSome() {
		if t := b.Some().Positionals.Tail; t.IsSome() && t.Some().Separator.IsSome() {
			if sep := t.Some().Separator.Some(); sep == "--help" || sep == "-h" {
				return false
			}
		}
	}
	for _, s := range namedSurfaces(c) {
		switch {
		case s.option != nil:
			if hasName(s.option.Long, s.option.Aliases, "help") || (s.option.Short.IsSome() && s.option.Short.Some() == 'h') {
				return false
			}
		case s.flag != nil:
			if hasName(s.flag.Long, s.flag.Aliases, "help") || (s.flag.Short.IsSome() && s.flag.Short.Some() == 'h') {
				return false
			}
		}
	}
	return true
}

func looksNegativeNumber(token string) bool {
	if len(token) < 2 {
		return false
	}
	c := token[1]
	return (c >= '0' && c <= '9') || c == '.'
}

type tokenKind uint8

const (
	tokHelp tokenKind = iota
	tokTailSeparator
	tokEndOptions
	tokLongOption
	tokLongFlag
	tokShortOptions
	tokSubcommand
	tokPositional
	tokTail
	tokTailStart
)

type token struct {
	kind     tokenKind
	verbatim bool
	option   *toolCommon.OptionSpec
	attached *string
	flag     *toolCommon.FlagSpec
	negated  bool
	inBody   bool
	child    Command
}

func (p *argParser) classify(tok string) (token, error) {
	body := p.body()
	if !p.tailStarted && body != nil && body.Positionals.Tail.IsSome() {
		optionalRest := true
		for _, fp := range body.Positionals.Fixed[min(len(p.positionals), len(body.Positionals.Fixed)):] {
			if fp.Required && fp.Default.IsNone() {
				optionalRest = false
			}
		}
		t := body.Positionals.Tail.Some()
		if optionalRest && t.Separator.IsSome() && t.Separator.Some() == tok {
			return token{kind: tokTailSeparator, verbatim: t.Verbatim}, nil
		}
	}
	implicit := !p.tailStarted && implicitTail(p.cmd)
	if p.optionsEnabled {
		named := p.named()
		if long, ok := strings.CutPrefix(tok, "--"); ok {
			name, value, hasValue := strings.Cut(long, "=")
			for _, s := range named {
				if s.option != nil && hasName(s.option.Long, s.option.Aliases, name) {
					t := token{kind: tokLongOption, option: s.option, inBody: s.inBody}
					if hasValue {
						t.attached = &value
					}
					return t, nil
				}
			}
			if f, negated, found := findLongFlag(named, name); found {
				if hasValue {
					return token{}, fmt.Errorf("flag --%s does not take a value", name)
				}
				inBody := false
				for _, s := range named {
					if s.flag == f {
						inBody = s.inBody
					}
				}
				return token{kind: tokLongFlag, flag: f, negated: negated, inBody: inBody}, nil
			}
		}
		// Declared short forms own their spelling, including digits and
		// punctuation. Where the tail can start without its separator, a
		// cluster belongs to the command only if each of its characters does.
		if cluster, ok := strings.CutPrefix(tok, "-"); ok && cluster != "" {
			first := []rune(cluster)[0]
			var own bool
			if implicit {
				own = declaredShortCluster(p.cmd, named, cluster)
			} else {
				_, own = findShort(named, first)
			}
			if own {
				return token{kind: tokShortOptions}, nil
			}
		}
		if !p.bodyStarted {
			if child, found := p.cmd.Subcommand(tok); found {
				return token{kind: tokSubcommand, child: child}, nil
			}
		}
		if helpAvailable(p.cmd) && (tok == "--help" || tok == "-h") {
			return token{kind: tokHelp}, nil
		}
		if tok == "--" {
			return token{kind: tokEndOptions}, nil
		}
		if implicit {
			return token{kind: tokTailStart}, nil
		}
		if strings.HasPrefix(tok, "--") && len(tok) > 2 {
			name, _, _ := strings.Cut(tok[2:], "=")
			return token{}, fmt.Errorf("unknown option --%s", name)
		}
		if strings.HasPrefix(tok, "-") && tok != "-" && !looksNegativeNumber(tok) {
			return token{kind: tokShortOptions}, nil
		}
	}
	if body == nil {
		return token{}, fmt.Errorf("expected a subcommand, found %q", tok)
	}
	if !p.tailStarted && len(p.positionals) < len(body.Positionals.Fixed) {
		return token{kind: tokPositional}, nil
	}
	if body.Positionals.Tail.IsSome() {
		t := body.Positionals.Tail.Some()
		if !p.tailStarted && t.Separator.IsSome() {
			return token{}, fmt.Errorf("expected tail separator %q", t.Separator.Some())
		}
		return token{kind: tokTail}, nil
	}
	return token{}, fmt.Errorf("unexpected argument %q", tok)
}

// step reads the token at p.i, and any value it takes, and reports whether it
// asks for help.
func (p *argParser) step(args []string) (bool, error) {
	tok := args[p.i]
	t, err := p.classify(tok)
	if err != nil {
		return false, err
	}
	switch t.kind {
	case tokHelp:
		return true, nil
	case tokTailSeparator:
		p.optionsEnabled = p.optionsEnabled && !t.verbatim
		p.tailStarted, p.bodyStarted = true, true
	case tokEndOptions:
		p.optionsEnabled = false
	case tokLongOption:
		value, err := p.optionValue(t.option, t.attached, args)
		if err != nil {
			return false, err
		}
		p.supplied[t.option.Long] = append(p.supplied[t.option.Long], value)
		p.bodyStarted = p.bodyStarted || t.inBody
	case tokLongFlag:
		if err := p.applyFlag(t.flag, t.negated); err != nil {
			return false, err
		}
		p.bodyStarted = p.bodyStarted || t.inBody
	case tokShortOptions:
		named := p.named()
		chars := []rune(tok[1:])
		for c := 0; c < len(chars); c++ {
			short := chars[c]
			if helpAvailable(p.cmd) && short == 'h' {
				return true, nil
			}
			s, found := findShort(named, short)
			if !found {
				return false, fmt.Errorf("unknown short option -%c", short)
			}
			if s.flag != nil {
				if err := p.applyFlag(s.flag, false); err != nil {
					return false, err
				}
				p.bodyStarted = p.bodyStarted || s.inBody
				continue
			}
			p.bodyStarted = p.bodyStarted || s.inBody
			var attached *string
			if rest := string(chars[c+1:]); rest != "" {
				attached = &rest
			}
			value, err := p.optionValue(s.option, attached, args)
			if err != nil {
				return false, err
			}
			p.supplied[s.option.Long] = append(p.supplied[s.option.Long], value)
			break
		}
	case tokSubcommand:
		p.cmd = t.child
	case tokPositional:
		p.bodyStarted = true
		p.positionals = append(p.positionals, tok)
	case tokTail:
		p.bodyStarted = true
		p.tail = append(p.tail, tok)
	case tokTailStart:
		p.optionsEnabled = false
		p.tailStarted, p.bodyStarted = true, true
		p.tail = append(p.tail, tok)
	}
	return false, nil
}

// bareValue marks an optional-value option written without its value.
const bareValue = "\x00"

func (p *argParser) optionValue(o *toolCommon.OptionSpec, attached *string, args []string) (string, error) {
	if attached != nil {
		return *attached, nil
	}
	if o.Shape.Tag() == toolCommon.OptionShapeOptionalScalar {
		return bareValue, nil
	}
	p.i++
	if p.i >= len(args) {
		return "", fmt.Errorf("--%s requires a value", o.Long)
	}
	return args[p.i], nil
}

func (p *argParser) applyFlag(f *toolCommon.FlagSpec, negated bool) error {
	st := p.flags[f.Long]
	if st == nil {
		st = &flagState{}
		p.flags[f.Long] = st
	}
	if f.Shape.Tag() == toolCommon.FlagShapeBoolFlag {
		v := !negated
		st.boolean = &v
		return nil
	}
	st.count++
	if max := f.Shape.CountFlag(); max.IsSome() && st.count > max.Some() {
		return fmt.Errorf("--%s exceeds maximum count", f.Long)
	}
	return nil
}

func boolWord(value string) (bool, bool) {
	for _, w := range []string{"1", "true", "yes", "on", "y", "t"} {
		if asciiEqualFold(value, w) {
			return true, true
		}
	}
	for _, w := range []string{"0", "false", "no", "off", "n", "f"} {
		if asciiEqualFold(value, w) {
			return false, true
		}
	}
	return false, false
}

// asciiEqualFold reports whether a and b are equal when ASCII letters are
// compared without regard to case. Other characters must match exactly, so
// that a word such as "yeſ" is not read as "yes".
func asciiEqualFold(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := 0; i < len(a); i++ {
		x, y := a[i], b[i]
		if 'A' <= x && x <= 'Z' {
			x += 'a' - 'A'
		}
		if 'A' <= y && y <= 'Z' {
			y += 'a' - 'A'
		}
		if x != y {
			return false
		}
	}
	return true
}

func flagFromVariable(f *toolCommon.FlagSpec, value string) (*flagState, error) {
	if f.Shape.Tag() == toolCommon.FlagShapeBoolFlag {
		v, ok := boolWord(value)
		if !ok {
			return nil, fmt.Errorf("invalid value %q: expected true, yes, on, y, t or 1, or false, no, off, n, f or 0", value)
		}
		return &flagState{boolean: &v}, nil
	}
	// A count reads as a decimal u32, which may have a leading '+'.
	count, err := strconv.ParseUint(strings.TrimPrefix(value, "+"), 10, 32)
	if err != nil {
		return nil, fmt.Errorf("invalid value %q: expected a count", value)
	}
	if max := f.Shape.CountFlag(); max.IsSome() && uint32(count) > max.Some() {
		return nil, fmt.Errorf("invalid value %q: exceeds maximum count", value)
	}
	return &flagState{count: uint32(count)}, nil
}

func declaredVariable(name witTypes.Option[string], variables map[string]string) (string, string, bool) {
	if !name.IsSome() {
		return "", "", false
	}
	v, found := variables[name.Some()]
	return name.Some(), v, found
}

func (p *argParser) finish(variables map[string]string) (Arguments, error) {
	body := p.body()
	if body == nil {
		return Arguments{}, errors.New("a subcommand is required")
	}
	input, err := p.cmd.input()
	if err != nil {
		return Arguments{}, err
	}
	fieldTypes := input.Type().Body.(core.RecordType).Fields
	all := surfaces(p.cmd)
	if len(all) != len(fieldTypes) {
		return Arguments{}, fmt.Errorf("golem: command %s has %d arguments but %d input fields",
			commandLabel(p.cmd.path), len(all), len(fieldTypes))
	}
	// An option or flag the command line left out takes its declared
	// variable, as if it had been written once.
	fromVariable := map[string]string{}
	for _, s := range all {
		switch {
		case s.option != nil:
			if _, given := p.supplied[s.option.Long]; given {
				continue
			}
			if name, value, found := declaredVariable(s.option.EnvVar, variables); found {
				p.supplied[s.option.Long] = []string{value}
				fromVariable[s.option.Long] = name
			}
		case s.flag != nil:
			if _, given := p.flags[s.flag.Long]; given {
				continue
			}
			if name, value, found := declaredVariable(s.flag.EnvVar, variables); found {
				st, err := flagFromVariable(s.flag, value)
				if err != nil {
					return Arguments{}, fmt.Errorf("--%s from variable %s: %w", s.flag.Long, name, err)
				}
				p.flags[s.flag.Long] = st
			}
		}
	}
	values := make([]core.SchemaValue, len(all))
	for i, s := range all {
		v, err := p.collect(input.At(fieldTypes[i].Body), s, body)
		if err != nil {
			if s.option != nil {
				if name, found := fromVariable[s.option.Long]; found {
					return Arguments{}, fmt.Errorf("--%s from variable %s: %w", s.option.Long, name, err)
				}
			}
			return Arguments{}, err
		}
		values[i] = v
	}
	if err := p.checkConstraints(body, all, fieldTypes, values); err != nil {
		return Arguments{}, err
	}
	record := core.RecordValue{Fields: values}
	if err := input.Validate(record); err != nil {
		return Arguments{}, fmt.Errorf("invalid tool arguments: %w", err)
	}
	unpacked, err := input.UnpackJSON(record)
	if err != nil {
		return Arguments{}, fmt.Errorf("invalid tool arguments: %w", err)
	}
	fields, _ := unpacked.(map[string]any)
	return Arguments{Path: p.cmd.Path(), Input: fields}, nil
}

func declaredDefault(d witTypes.Option[types.SchemaValueTree]) (core.SchemaValue, error) {
	if !d.IsSome() {
		return nil, nil
	}
	return witschema.ValueToCore(d.Some())
}

func (p *argParser) collect(field core.Ref, s argSurface, body *toolCommon.CommandBody) (core.SchemaValue, error) {
	switch {
	case s.tail:
		t := body.Positionals.Tail.Some()
		n := uint32(len(p.tail))
		if n < t.Min || (t.Max.IsSome() && n > t.Max.Some()) {
			return nil, fmt.Errorf("tail %s has invalid cardinality", t.Name)
		}
		item := field.At(field.Type().Body.(core.ListType).Element)
		items := make([]core.SchemaValue, 0, len(p.tail))
		for _, raw := range p.tail {
			v, err := decodeArg(item, raw)
			if err != nil {
				return nil, err
			}
			items = append(items, v)
		}
		return core.ListValue{Items: items}, nil
	case s.flag != nil:
		st := p.flags[s.flag.Long]
		if s.flag.Shape.Tag() == toolCommon.FlagShapeBoolFlag {
			if st != nil && st.boolean != nil {
				return core.BoolValue{Value: *st.boolean}, nil
			}
			return core.BoolValue{Value: s.flag.Shape.BoolFlag().Default}, nil
		}
		if st == nil {
			return core.U32Value{Value: 0}, nil
		}
		return core.U32Value{Value: st.count}, nil
	case s.option != nil:
		raw, given := p.supplied[s.option.Long]
		if !given {
			def, err := declaredDefault(s.option.Default)
			if err != nil {
				return nil, err
			}
			if def != nil {
				return def, nil
			}
			if s.option.Required {
				return nil, fmt.Errorf("missing required option --%s", s.option.Long)
			}
		}
		return collectOption(field, s.option, raw)
	default:
		fp := body.Positionals.Fixed[s.positional]
		if s.positional < len(p.positionals) {
			return decodeArg(field, p.positionals[s.positional])
		}
		def, err := declaredDefault(fp.Default)
		if err != nil {
			return nil, err
		}
		switch {
		case def != nil:
			return def, nil
		case !fp.Required:
			return omitted(field)
		default:
			return nil, fmt.Errorf("missing required positional %s", fp.Name)
		}
	}
}

func collectOption(field core.Ref, o *toolCommon.OptionSpec, raw []string) (core.SchemaValue, error) {
	repeatable := false
	switch o.Shape.Tag() {
	case toolCommon.OptionShapeRepeatableList:
		repeatable = o.Shape.RepeatableList().Repetition.Tag() != toolCommon.RepetitionDelimited
	case toolCommon.OptionShapeRepeatableMap:
		repeatable = o.Shape.RepeatableMap().Repetition.Tag() != toolCommon.RepetitionDelimited
	}
	if len(raw) > 1 && !repeatable {
		return nil, fmt.Errorf("--%s cannot be repeated", o.Long)
	}
	switch o.Shape.Tag() {
	case toolCommon.OptionShapeScalar:
		if len(raw) == 0 {
			return omitted(field)
		}
		return decodeArg(field, raw[len(raw)-1])
	case toolCommon.OptionShapeOptionalScalar:
		if len(raw) == 0 {
			return omitted(field)
		}
		if last := raw[len(raw)-1]; last != bareValue {
			return decodeArg(field, last)
		}
		def, err := declaredDefault(o.Default)
		if err != nil {
			return nil, err
		}
		if def == nil {
			return nil, fmt.Errorf("--%s has no bare-value default", o.Long)
		}
		return def, nil
	case toolCommon.OptionShapeRepeatableList:
		item := field.At(field.Type().Body.(core.ListType).Element)
		parts := expand(raw, o.Shape.RepeatableList().Repetition)
		items := make([]core.SchemaValue, 0, len(parts))
		for _, part := range parts {
			v, err := decodeArg(item, part)
			if err != nil {
				return nil, err
			}
			items = append(items, v)
		}
		return core.ListValue{Items: items}, nil
	default:
		shape := o.Shape.RepeatableMap()
		resolved, err := field.Resolved()
		if err != nil {
			return nil, err
		}
		mt, isMap := resolved.Type().Body.(core.MapType)
		if !isMap {
			return nil, fmt.Errorf("--%s does not declare a map type", o.Long)
		}
		var entries []core.MapEntry
		for _, item := range expand(raw, shape.Repetition) {
			k, v, found := strings.Cut(item, "=")
			if !found {
				return nil, fmt.Errorf("--%s expects KEY=VALUE", o.Long)
			}
			key, err := decodeArg(field.At(mt.Key), k)
			if err != nil {
				return nil, err
			}
			value, err := decodeArg(field.At(mt.Value), v)
			if err != nil {
				return nil, err
			}
			replaced := false
			for i := range entries {
				if reflect.DeepEqual(entries[i].Key, key) {
					if shape.DuplicateKeyPolicy == toolCommon.DuplicateKeyPolicyReject {
						return nil, fmt.Errorf("duplicate key for --%s", o.Long)
					}
					entries[i] = core.MapEntry{Key: key, Value: value}
					replaced = true
					break
				}
			}
			if !replaced {
				entries = append(entries, core.MapEntry{Key: key, Value: value})
			}
		}
		return core.MapValue{Entries: entries}, nil
	}
}

func expand(values []string, r toolCommon.Repetition) []string {
	var sep rune
	switch r.Tag() {
	case toolCommon.RepetitionDelimited:
		sep = r.Delimited()
	case toolCommon.RepetitionEither:
		sep = r.Either()
	default:
		return values
	}
	var out []string
	for _, v := range values {
		out = append(out, strings.Split(v, string(sep))...)
	}
	return out
}

func omitted(field core.Ref) (core.SchemaValue, error) {
	resolved, err := field.Resolved()
	if err != nil {
		return nil, err
	}
	switch resolved.Type().Body.(type) {
	case core.OptionType:
		return core.OptionValue{}, nil
	case core.ListType:
		return core.ListValue{}, nil
	case core.MapType:
		return core.MapValue{}, nil
	}
	return nil, errors.New("optional argument must have an option or collection schema, or a default")
}

// decodeArg reads one command-line word as a value of t. Text-like values go
// in as they are written, a 64-bit integer as its digits, an enum case in any
// letter case when that names one case, and anything else as JSON.
func decodeArg(t core.Ref, raw string) (core.SchemaValue, error) {
	resolved, err := t.Resolved()
	if err != nil {
		return nil, err
	}
	var value any
	switch b := resolved.Type().Body.(type) {
	case core.OptionType:
		inner, err := decodeArg(t.At(b.Inner), raw)
		if err != nil {
			return nil, err
		}
		return core.OptionValue{Value: &inner}, nil
	case core.StringType, core.CharType, core.PathType, core.UrlType, core.UUIDType,
		core.DatetimeType, core.DurationType:
		value = raw
	case core.EnumType:
		value = enumCase(b.Cases, raw)
	case core.BoolType:
		switch {
		case asciiEqualFold(raw, "true"):
			value = true
		case asciiEqualFold(raw, "false"):
			value = false
		default:
			value, err = parseJSON(raw)
		}
	case core.U64Type, core.S64Type:
		if strings.HasPrefix(raw, `"`) {
			value, err = parseJSON(raw)
		} else {
			value = raw
		}
	default:
		value, err = parseJSON(raw)
	}
	if err != nil {
		return nil, fmt.Errorf("invalid value %q: %w", raw, err)
	}
	v, err := t.PackJSON(value)
	if err != nil {
		return nil, fmt.Errorf("invalid value %q: %w", raw, err)
	}
	return v, nil
}

func parseJSON(raw string) (any, error) {
	dec := json.NewDecoder(bytes.NewReader([]byte(raw)))
	dec.UseNumber()
	var value any
	if err := dec.Decode(&value); err != nil {
		return nil, err
	}
	if _, err := dec.Token(); err != io.EOF {
		return nil, errors.New("trailing characters")
	}
	return value, nil
}

// enumCase returns the case raw names: the exact one, or the only one equal
// to it without regard to letter case. Other text goes on unchanged, and the
// schema refuses it.
func enumCase(cases []string, raw string) string {
	var equal []string
	for _, c := range cases {
		if c == raw {
			return raw
		}
		if asciiEqualFold(c, raw) {
			equal = append(equal, c)
		}
	}
	if len(equal) == 1 {
		return equal[0]
	}
	return raw
}

func (p *argParser) checkConstraints(body *toolCommon.CommandBody, all []argSurface,
	fields []core.NamedField, values []core.SchemaValue) error {
	find := func(name string) (int, bool) {
		for i, s := range all {
			if fields[i].Name == name {
				return i, true
			}
			switch {
			case s.option != nil && hasName(s.option.Long, s.option.Aliases, name):
				return i, true
			case s.flag != nil && hasName(s.flag.Long, s.flag.Aliases, name):
				return i, true
			}
		}
		return 0, false
	}
	var refErr error
	matches := func(ref toolCommon.Ref) bool {
		if ref.Tag() == toolCommon.RefValueIs {
			v := ref.ValueIs()
			i, found := find(v.Name)
			if !found {
				return false
			}
			expected, err := witschema.ValueToCore(v.Value)
			if err != nil {
				refErr = err
				return false
			}
			return valueMatches(values[i], expected)
		}
		i, found := find(ref.Present())
		if !found {
			return false
		}
		return surfacePresent(all[i], body, values[i])
	}
	quantified := func(q toolCommon.Quantifier, refs []toolCommon.Ref) bool {
		if q == toolCommon.QuantifierAll {
			for _, r := range refs {
				if !matches(r) {
					return false
				}
			}
			return true
		}
		for _, r := range refs {
			if matches(r) {
				return true
			}
		}
		return false
	}
	for index, c := range body.Constraints {
		var satisfied bool
		switch c.Tag() {
		case toolCommon.ConstraintRequiresAll:
			satisfied = quantified(toolCommon.QuantifierAll, c.RequiresAll())
		case toolCommon.ConstraintAllOrNone:
			refs := c.AllOrNone()
			present := 0
			for _, r := range refs {
				if matches(r) {
					present++
				}
			}
			satisfied = present == 0 || present == len(refs)
		case toolCommon.ConstraintRequiresAny:
			satisfied = quantified(toolCommon.QuantifierAny, c.RequiresAny())
		case toolCommon.ConstraintMutexGroups:
			groups := 0
			for _, g := range c.MutexGroups() {
				if quantified(toolCommon.QuantifierAll, g.Refs) {
					groups++
				}
			}
			satisfied = groups <= 1
		case toolCommon.ConstraintImplies:
			i := c.Implies()
			satisfied = !quantified(i.LhsQuant, i.Lhs) || quantified(i.RhsQuant, i.Rhs)
		case toolCommon.ConstraintForbids:
			f := c.Forbids()
			satisfied = !quantified(f.LhsQuant, f.Lhs) || !quantified(toolCommon.QuantifierAny, f.Rhs)
		default:
			return fmt.Errorf("golem: unknown tool constraint %d", c.Tag())
		}
		if refErr != nil {
			return refErr
		}
		if !satisfied {
			return fmt.Errorf("tool command constraint %d is not satisfied", index)
		}
	}
	return nil
}

func valueMatches(value, expected core.SchemaValue) bool {
	if reflect.DeepEqual(value, expected) {
		return true
	}
	switch v := value.(type) {
	case core.OptionValue:
		return v.Value != nil && valueMatches(*v.Value, expected)
	case core.ListValue:
		for _, item := range v.Items {
			if valueMatches(item, expected) {
				return true
			}
		}
	case core.FixedListValue:
		for _, item := range v.Items {
			if valueMatches(item, expected) {
				return true
			}
		}
	case core.MapValue:
		for _, e := range v.Entries {
			if valueMatches(e.Value, expected) {
				return true
			}
		}
	}
	return false
}

func surfacePresent(s argSurface, body *toolCommon.CommandBody, value core.SchemaValue) bool {
	switch {
	case s.flag != nil:
		if s.flag.Shape.Tag() == toolCommon.FlagShapeBoolFlag {
			b, ok := value.(core.BoolValue)
			return ok && b.Value != s.flag.Shape.BoolFlag().Default
		}
		n, ok := value.(core.U32Value)
		return ok && n.Value != 0
	case s.option != nil:
		def, _ := declaredDefault(s.option.Default)
		return valuePresent(value, def)
	case s.tail:
		return valuePresent(value, nil)
	default:
		def, _ := declaredDefault(body.Positionals.Fixed[s.positional].Default)
		return valuePresent(value, def)
	}
}

func valuePresent(value, def core.SchemaValue) bool {
	if def != nil && reflect.DeepEqual(value, def) {
		return false
	}
	switch v := value.(type) {
	case core.OptionValue:
		return v.Value != nil
	case core.ListValue:
		return len(v.Items) != 0
	case core.FixedListValue:
		return len(v.Items) != 0
	case core.MapValue:
		return len(v.Entries) != 0
	case core.BoolValue:
		return v.Value
	case core.U32Value:
		return v.Value != 0
	}
	return true
}

func (p *argParser) renderHelp() string {
	c := p.cmd
	n := c.node()
	var out strings.Builder
	out.WriteString("Usage: ")
	out.WriteString(p.tool.wit.Commands.Nodes[0].Name)
	for _, part := range c.path {
		out.WriteString(" " + part)
	}
	named := namedSurfaces(c)
	if len(named) != 0 {
		out.WriteString(" [OPTIONS]")
	}
	if len(n.Subcommands) != 0 {
		out.WriteString(" [COMMAND]")
	}
	implicit := implicitTail(c)
	if n.Body.IsSome() {
		body := n.Body.Some()
		for _, fp := range body.Positionals.Fixed {
			out.WriteString(" " + positionalLabel(fp))
		}
		if body.Positionals.Tail.IsSome() {
			t := body.Positionals.Tail.Some()
			if t.Separator.IsSome() {
				if implicit {
					out.WriteString(" [" + t.Separator.Some() + "]")
				} else {
					out.WriteString(" " + t.Separator.Some())
				}
			}
			out.WriteString(" " + tailLabel(t))
		}
	}
	out.WriteString("\n")
	if n.Doc.Summary != "" {
		out.WriteString("\n" + n.Doc.Summary + "\n")
	}
	if n.Doc.Description != "" {
		out.WriteString(n.Doc.Description + "\n")
	}
	if len(n.Subcommands) != 0 {
		out.WriteString("\nCommands:\n")
		for _, idx := range n.Subcommands {
			if int(idx) >= 0 && int(idx) < len(c.tree.Nodes) {
				child := c.tree.Nodes[idx]
				fmt.Fprintf(&out, "  %-16s %s\n", child.Name, child.Doc.Summary)
			}
		}
	}
	if n.Body.IsSome() {
		body := n.Body.Some()
		type argument struct {
			label, summary, start string
			node                  int32
		}
		var arguments []argument
		for _, fp := range body.Positionals.Fixed {
			arguments = append(arguments, argument{positionalLabel(fp), fp.Doc.Summary, "", fp.Type})
		}
		if body.Positionals.Tail.IsSome() {
			t := body.Positionals.Tail.Some()
			start := ""
			if implicit {
				start = " [starts at the first argument that is not an option of this command, or after --]"
			}
			arguments = append(arguments, argument{tailLabel(t), t.Doc.Summary, start, t.ItemType})
		}
		if len(arguments) != 0 {
			out.WriteString("\nArguments:\n")
		}
		for _, a := range arguments {
			text := a.summary + c.possibleValues(a.node) + a.start
			line := fmt.Sprintf("  %-16s %s", a.label, strings.TrimLeftFunc(text, unicode.IsSpace))
			out.WriteString(strings.TrimRightFunc(line, unicode.IsSpace) + "\n")
		}
	}
	out.WriteString("\nOptions:\n")
	for _, s := range named {
		if o := s.option; o != nil {
			short := ""
			if o.Short.IsSome() {
				short = fmt.Sprintf("-%c, ", o.Short.Some())
			}
			value := "VALUE"
			if o.ValueName.IsSome() {
				value = o.ValueName.Some()
			}
			if o.Shape.Tag() == toolCommon.OptionShapeOptionalScalar {
				value = "[=" + value + "]"
			} else {
				value = " <" + value + ">"
			}
			fmt.Fprintf(&out, "  %s--%s%s  %s", short, o.Long, value, o.Doc.Summary)
			if o.Required {
				out.WriteString(" [required]")
			}
			if o.Default.IsSome() {
				out.WriteString(" [has default]")
			}
			if len(o.Aliases) != 0 {
				out.WriteString(" [aliases: " + strings.Join(o.Aliases, ", ") + "]")
			}
			switch o.Shape.Tag() {
			case toolCommon.OptionShapeScalar:
				out.WriteString(c.possibleValues(o.Shape.Scalar()))
			case toolCommon.OptionShapeOptionalScalar:
				out.WriteString(c.possibleValues(o.Shape.OptionalScalar()))
			case toolCommon.OptionShapeRepeatableList:
				out.WriteString(c.possibleValues(o.Shape.RepeatableList().ItemType))
			}
			out.WriteString("\n")
			continue
		}
		f := s.flag
		short := ""
		if f.Short.IsSome() {
			short = fmt.Sprintf("-%c, ", f.Short.Some())
		}
		fmt.Fprintf(&out, "  %s--%s  %s", short, f.Long, f.Doc.Summary)
		if f.Shape.Tag() == toolCommon.FlagShapeBoolFlag && f.Shape.BoolFlag().Negatable {
			out.WriteString(" [--no-" + f.Long + "]")
		}
		if len(f.Aliases) != 0 {
			out.WriteString(" [aliases: " + strings.Join(f.Aliases, ", ") + "]")
		}
		out.WriteString("\n")
	}
	if helpAvailable(c) {
		out.WriteString("  -h, --help      Print help (reserved only when undeclared)\n")
	}
	return out.String()
}

func positionalLabel(fp toolCommon.Positional) string {
	name := fp.Name
	if fp.ValueName.IsSome() {
		name = fp.ValueName.Some()
	}
	if fp.Required && fp.Default.IsNone() {
		return "<" + name + ">"
	}
	return "[" + name + "]"
}

func tailLabel(t toolCommon.TailPositional) string {
	name := t.Name
	if t.ValueName.IsSome() {
		name = t.ValueName.Some()
	}
	return "[" + name + "]..."
}

// possibleValues is the help marker that lists the cases of an enum, through
// named references and options, or nothing for another type.
func (c Command) possibleValues(node int32) string {
	t, err := c.conv.At(node)
	if err != nil {
		return ""
	}
	ref := core.NewRefAt(c.conv.Graph, t)
	// Bounded, because a recursive type can contain itself through an option.
	for range 8 {
		resolved, err := ref.Resolved()
		if err != nil {
			return ""
		}
		switch b := resolved.Type().Body.(type) {
		case core.EnumType:
			return " [possible values: " + strings.Join(b.Cases, ", ") + "]"
		case core.OptionType:
			ref = ref.At(b.Inner)
		default:
			return ""
		}
	}
	return ""
}

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
	"reflect"
)

// Tool arguments.
//
// A command's input is a Go struct whose every exported field is one of the
// argument markers below. The marker carries two things at once: the CLI
// metadata, written once in a prototype literal at declaration time, and the
// parsed value, filled in on the copy the handler receives.
//
//	type GreetArgs struct {
//	    Name  golem.Positional[string]
//	    Loud  golem.Flag
//	    Times golem.Opt[int32]
//	}
//
//	var Greet = Greeter.Command[GreetArgs, string]("greet", GreetArgs{
//	    Name:  golem.Positional[string]{Doc: "who to greet"},
//	    Loud:  golem.Flag{Short: 'l', Doc: "shout the greeting"},
//	    Times: golem.Opt[int32]{Short: 'n', Default: Some(int32(1))},
//	})
//
//	func greet(ctx *golem.ToolContext, in GreetArgs) string {
//	    if in.Loud.Get() { ... }
//	    return strings.Repeat("hi "+in.Name.Get(), int(in.Times.Get()))
//	}
//
// Fields that need no customisation are left out of the prototype: the zero
// value means no short form, no documentation, and no default. The wire name of
// each argument is its field name with the first letter lower-cased, the same
// rule record fields follow.
//
// Note the distinction from [DefineFlags]: that registers a WIT flags *type*, a
// set of named booleans travelling as one value. A [Flag] here is a command-line
// switch.

// toolArgKind separates the three argument positions a field can occupy.
type toolArgKind uint8

const (
	argPositional toolArgKind = iota
	argOption
	argFlag
)

func (k toolArgKind) String() string {
	switch k {
	case argPositional:
		return "positional"
	case argOption:
		return "option"
	default:
		return "flag"
	}
}

// toolArgMeta is the declaration-time metadata read off a prototype field.
type toolArgMeta struct {
	doc          string
	valueName    string
	short        rune
	aliases      []string
	envVar       string
	required     bool
	acceptsStdin bool
	// def is the declared default, invalid when there is none.
	def reflect.Value
}

// toolArg is implemented by every argument marker, so the deriver can read a
// field's kind, payload type and metadata without knowing which marker it is.
type toolArg interface {
	toolArgKind() toolArgKind
	toolArgElem() reflect.Type
	toolArgMeta() toolArgMeta
}

// toolArgSetter fills in a parsed value. It is implemented on the pointer
// receiver, so decoding addresses the field.
type toolArgSetter interface{ toolArgSet(v reflect.Value) }

// Positional is a command argument identified by its position rather than by a
// name. Positionals are required unless Optional is set, and only the last one
// on a command may be variadic.
type Positional[T any] struct {
	Doc string
	// ValueName is the placeholder shown in help text, e.g. "FILE". Defaults to
	// the field name.
	ValueName string
	// Optional allows the argument to be omitted. Positionals are required by
	// default, which is the common case.
	Optional bool
	// AcceptsStdin allows the value to be read from standard input when it is
	// not given on the command line.
	AcceptsStdin bool
	// Default supplies the value when the argument is omitted.
	Default Option[T]

	value T
}

// Get returns the parsed value.
func (p Positional[T]) Get() T { return p.value }

func (Positional[T]) toolArgKind() toolArgKind      { return argPositional }
func (Positional[T]) toolArgElem() reflect.Type     { return reflect.TypeFor[T]() }
func (p *Positional[T]) toolArgSet(v reflect.Value) { p.value = v.Interface().(T) }

func (p Positional[T]) toolArgMeta() toolArgMeta {
	return toolArgMeta{
		doc:          p.Doc,
		valueName:    p.ValueName,
		required:     !p.Optional,
		acceptsStdin: p.AcceptsStdin,
		def:          optionValue(p.Default),
	}
}

// Opt is a named argument carrying a value, written --name value. It is
// optional unless Required is set.
type Opt[T any] struct {
	Doc string
	// ValueName is the placeholder shown in help text, e.g. "COUNT". Defaults to
	// the field name.
	ValueName string
	// Short is the single-character form, e.g. 'n' for -n. Zero means none.
	Short rune
	// Aliases are additional long names accepted for this option.
	Aliases []string
	// EnvVar names an environment variable consulted when the option is absent.
	EnvVar string
	// Required rejects an invocation that omits the option.
	Required bool
	// Default supplies the value when the option is absent.
	Default Option[T]

	value T
}

// Get returns the parsed value, which is the declared default when the option
// was not supplied.
func (o Opt[T]) Get() T { return o.value }

func (Opt[T]) toolArgKind() toolArgKind      { return argOption }
func (Opt[T]) toolArgElem() reflect.Type     { return reflect.TypeFor[T]() }
func (o *Opt[T]) toolArgSet(v reflect.Value) { o.value = v.Interface().(T) }

func (o Opt[T]) toolArgMeta() toolArgMeta {
	return toolArgMeta{
		doc:       o.Doc,
		valueName: o.ValueName,
		short:     o.Short,
		aliases:   o.Aliases,
		envVar:    o.EnvVar,
		required:  o.Required,
		def:       optionValue(o.Default),
	}
}

// Flag is a named boolean switch, written --name with no value. A flag is never
// required: its absence is false.
type Flag struct {
	Doc string
	// Short is the single-character form, e.g. 'l' for -l. Zero means none.
	Short rune
	// Aliases are additional long names accepted for this flag.
	Aliases []string
	// EnvVar names an environment variable consulted when the flag is absent.
	EnvVar string

	value bool
}

// Get reports whether the flag was set.
func (f Flag) Get() bool { return f.value }

func (Flag) toolArgKind() toolArgKind      { return argFlag }
func (Flag) toolArgElem() reflect.Type     { return reflect.TypeFor[bool]() }
func (f *Flag) toolArgSet(v reflect.Value) { f.value = v.Bool() }

func (f Flag) toolArgMeta() toolArgMeta {
	return toolArgMeta{doc: f.Doc, short: f.Short, aliases: f.Aliases, envVar: f.EnvVar}
}

// optionValue turns a declared Option[T] default into a reflect.Value, invalid
// when the option is empty, so all three markers report defaults the same way.
func optionValue[T any](o Option[T]) reflect.Value {
	v, some := o.optionGet()
	if !some {
		return reflect.Value{}
	}
	return v
}

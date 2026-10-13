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

package schema

import (
	"errors"
	"fmt"
)

// Parameter lists.
//
// An invocation's input is not a single value but an ordered, named parameter
// list, which travels as one record whose fields are the parameters in
// declaration order. A reflective caller supplies them by name, since it has no
// Go type for the target's signature.

// BoolParameter describes a parameter whose type is fixed as boolean rather
// than named in the graph. A tool flag is the case that needs it: the wire
// gives a flag no type of its own because its type can only be bool, so a graph
// carrying flags need not describe one.
func BoolParameter(name string) Parameter {
	return Parameter{Name: name, Type: SchemaType{Body: BoolType{}}, fixedBool: true}
}

// Parameter is one field of a named parameter list.
type Parameter struct {
	Name string
	Type SchemaType
	// fixedBool marks a parameter whose type the wire fixes rather than names.
	fixedBool bool
}

// Issue is one problem found in a value, with the path that leads to it.
type Issue struct {
	Path    string
	Message string
}

// ValidationError collects every problem found, rather than stopping at the
// first: a caller assembling arguments should learn about all of its mistakes
// at once.
type ValidationError struct{ Issues []Issue }

func (e *ValidationError) Error() string {
	if len(e.Issues) == 1 {
		return e.Issues[0].Path + ": " + e.Issues[0].Message
	}
	msg := fmt.Sprintf("%d problems:", len(e.Issues))
	for _, i := range e.Issues {
		msg += "\n  - " + i.Path + ": " + i.Message
	}
	return msg
}

// PackParameters builds the record an invocation carries: the parameters in
// declaration order, each packed against its own type.
//
// A missing argument is a problem, and so is one the list does not declare, so
// a caller learns about a typo here rather than at the target. Every problem is
// reported together.
func (r Ref) PackParameters(params []Parameter, args map[string]any) (SchemaValue, error) {
	var issues []Issue
	for name := range args {
		if !hasParameter(params, name) {
			issues = append(issues, Issue{Path: name, Message: "unexpected argument"})
		}
	}

	fields := make([]SchemaValue, 0, len(params))
	for _, param := range params {
		value, given := args[param.Name]
		if !given {
			issues = append(issues, Issue{Path: param.Name, Message: "missing argument"})
			continue
		}
		built, err := r.At(param.Type).PackJSON(value)
		if err != nil {
			var ve *ValidationError
			if errors.As(err, &ve) {
				issues = append(issues, ve.Issues...)
				continue
			}
			issues = append(issues, Issue{Path: param.Name, Message: err.Error()})
			continue
		}
		fields = append(fields, built)
	}
	if len(issues) > 0 {
		return nil, &ValidationError{Issues: issues}
	}
	return RecordValue{Fields: fields}, nil
}

// UnpackParameters is the inverse, reading an invocation's record back into
// named arguments.
func (r Ref) UnpackParameters(params []Parameter, v SchemaValue) (map[string]any, error) {
	record, ok := v.(RecordValue)
	if !ok {
		return nil, fmt.Errorf("golem: expected a record at the root of the parameter list, got %T", v)
	}
	if len(record.Fields) != len(params) {
		return nil, fmt.Errorf("golem: parameter list has %d value(s), want %d",
			len(record.Fields), len(params))
	}
	out := make(map[string]any, len(params))
	for i, param := range params {
		value, err := r.At(param.Type).UnpackJSON(record.Fields[i])
		if err != nil {
			return nil, fmt.Errorf("golem: parameter %q: %w", param.Name, err)
		}
		out[param.Name] = value
	}
	return out, nil
}

// ParametersJSONSchema renders a parameter list as a JSON Schema object, which
// is the shape a model or a form is given to fill in.
func (r Ref) ParametersJSONSchema(params []Parameter, includeDraftMarker bool) (any, error) {
	props := obj{}
	required := make([]string, 0, len(params))
	for _, param := range params {
		rendered, err := r.renderSchema(param.Type)
		if err != nil {
			return nil, err
		}
		props[param.Name] = rendered
		if param.fixedBool {
			required = append(required, param.Name)
			continue
		}
		optional, err := r.resolvesToOption(param.Type)
		if err != nil {
			return nil, err
		}
		if !optional {
			required = append(required, param.Name)
		}
	}
	out := obj{
		"type":                 "object",
		"properties":           props,
		"required":             requiredList(required),
		"additionalProperties": false,
	}
	if includeDraftMarker {
		out["$schema"] = jsonSchemaDraft
	}
	defs, err := r.renderDefs()
	if err != nil {
		return nil, err
	}
	if len(defs) > 0 {
		out["$defs"] = defs
	}
	return out, nil
}

func hasParameter(params []Parameter, name string) bool {
	for _, p := range params {
		if p.Name == name {
			return true
		}
	}
	return false
}

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
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"unicode/utf8"
)

// WireStreamRef is a stream as an invocation session names it on the wire:
// a provisional reference the client chose before the server accepted the
// invocation, or the stable token the server assigned. Exactly one is set.
type WireStreamRef struct {
	ProvisionalRef string
	StreamToken    string
}

// Release does nothing: the reference is a name, and the session that issued
// it owns the stream.
func (WireStreamRef) Release() {}

// The limits the public protocol freezes for any JSON it carries.
const (
	maxWireJSONDepth      = 64
	maxWireJSONCollection = 100_000
)

// CheckWireJSON enforces what the public protocol requires of every JSON text
// before it is interpreted: valid UTF-8, exactly one value, no member name
// repeated within an object, at most 64 levels of nesting and at most 100,000
// members or elements in one collection. encoding/json accepts duplicates
// (the last wins) and invalid UTF-8 (replaced), so this runs first.
func CheckWireJSON(data []byte) error {
	if !utf8.Valid(data) {
		return errors.New("not valid UTF-8")
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	type frame struct {
		object bool
		keys   map[string]bool
		count  int
		// expectKey is true in an object when the next token is a member name.
		expectKey bool
	}
	var stack []*frame
	first := true
	for {
		tok, err := dec.Token()
		if err == io.EOF {
			if first {
				return errors.New("empty JSON text")
			}
			return nil
		}
		if err != nil {
			return err
		}
		if len(stack) == 0 && !first {
			return errors.New("trailing data after the JSON value")
		}
		first = false
		var top *frame
		if len(stack) > 0 {
			top = stack[len(stack)-1]
		}
		if top != nil && top.object && top.expectKey {
			if d, isDelim := tok.(json.Delim); isDelim && d == '}' {
				stack = stack[:len(stack)-1]
				continue
			}
			key, ok := tok.(string)
			if !ok {
				return errors.New("expected an object member name")
			}
			if top.keys[key] {
				return fmt.Errorf("duplicate object member %q", key)
			}
			top.keys[key] = true
			top.count++
			if top.count > maxWireJSONCollection {
				return errors.New("an object exceeds the protocol's collection limit")
			}
			top.expectKey = false
			continue
		}
		if top != nil && !top.object {
			if d, isDelim := tok.(json.Delim); isDelim && d == ']' {
				stack = stack[:len(stack)-1]
				continue
			}
			top.count++
			if top.count > maxWireJSONCollection {
				return errors.New("an array exceeds the protocol's collection limit")
			}
		}
		if top != nil && top.object {
			top.expectKey = true
		}
		if d, isDelim := tok.(json.Delim); isDelim {
			if len(stack)+1 > maxWireJSONDepth {
				return errors.New("JSON nests deeper than the protocol allows")
			}
			switch d {
			case '{':
				stack = append(stack, &frame{object: true, keys: map[string]bool{}, expectKey: true})
			case '[':
				stack = append(stack, &frame{})
			}
		}
	}
}

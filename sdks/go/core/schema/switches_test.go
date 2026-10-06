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
	"strings"
	"testing"
)

// Go type switches are not exhaustive: a type kind added to the model but
// missing from one of them falls through to its fallback at run time. Every
// switch over SchemaTypeBody is checked here against everyTypeKind(), whose
// length TestEveryTypeKindIsAccountedFor pins to the model, so a new kind
// fails this test until each switch handles it — even if only to refuse it.
func TestEverySchemaSwitchHandlesEveryTypeKind(t *testing.T) {
	fallbacks := []string{"unsupported schema type", "cannot render"}
	reachedFallback := func(err error) bool {
		if err == nil {
			return false
		}
		for _, f := range fallbacks {
			if strings.Contains(err.Error(), f) {
				return true
			}
		}
		return false
	}
	graph := SchemaGraph{Defs: []SchemaTypeDef{{Id: "t1", Body: SchemaType{Body: BoolType{}}}}}
	for _, kind := range everyTypeKind() {
		ref := NewRef(SchemaGraph{Defs: graph.Defs, Root: SchemaType{Body: kind.want}})
		// A value of the wrong shape is fine: each switch must reject it from
		// its own case, not from its fallback.
		if err := ref.Validate(BoolValue{}); reachedFallback(err) {
			t.Errorf("Validate has no case for %T: %v", kind.want, err)
		}
		if _, err := ref.UnpackJSON(BoolValue{}); reachedFallback(err) {
			t.Errorf("UnpackJSON has no case for %T: %v", kind.want, err)
		}
		if _, err := ref.PackJSON(true); reachedFallback(err) {
			t.Errorf("PackJSON has no case for %T: %v", kind.want, err)
		}
		if _, err := ref.ToJSONSchema(false); reachedFallback(err) {
			t.Errorf("ToJSONSchema has no case for %T: %v", kind.want, err)
		}
	}
}

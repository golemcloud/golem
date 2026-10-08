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

// Host-managed handles.
//
// Four kinds of value are not data: they are capabilities the platform owns and
// lends out. This module cannot know how any of them is represented — inside a
// component they are WebAssembly resources, outside one they are ids on a wire
// — so it holds only the narrow interface each must satisfy, and the ownership
// rules that apply to all of them.
//
// A handle is affine: it is transferred, not copied. Whoever holds one must
// eventually release it, and a value that is abandoned part-way through being
// built must release the handles it had already taken, or they leak.

// Handle is what every host-managed value has in common.
type Handle interface {
	// Release gives the handle back. It must be safe to call more than once.
	Release()
}

// SecretHandle is a reference to a secret the platform holds.
type SecretHandle interface{ Handle }

// QuotaTokenHandle is a reference to a spending allowance.
type QuotaTokenHandle interface{ Handle }

// PermissionCardHandle is a reference to an authority the host granted.
type PermissionCardHandle interface{ Handle }

// StreamHandle is a reference to a sequence that arrives over time.
type StreamHandle interface{ Handle }

// ReleaseAll releases every handle reachable from v.
//
// It exists because a value is built one piece at a time: if a later field
// fails to convert, the handles taken for the earlier ones are already out of
// the platform's hands and nothing else will give them back. Call it on the
// partially-built value and the leak is closed.
func ReleaseAll(v SchemaValue) {
	switch t := v.(type) {
	case SecretValue:
		release(t.Handle)
	case QuotaTokenValue:
		release(t.Handle)
	case PermissionCardValue:
		release(t.Handle)
	case StreamValue:
		release(t.Handle)

	case RecordValue:
		releaseEach(t.Fields)
	case TupleValue:
		releaseEach(t.Elements)
	case ListValue:
		releaseEach(t.Items)
	case FixedListValue:
		releaseEach(t.Items)
	case MapValue:
		for _, e := range t.Entries {
			ReleaseAll(e.Key)
			ReleaseAll(e.Value)
		}
	case VariantValue:
		releaseOne(t.Payload)
	case OptionValue:
		releaseOne(t.Value)
	case ResultValue:
		releaseOne(t.Value)
	case UnionValue:
		ReleaseAll(t.Body)
	}
}

func release(h Handle) {
	if h != nil {
		h.Release()
	}
}

func releaseOne(v *SchemaValue) {
	if v != nil {
		ReleaseAll(*v)
	}
}

func releaseEach(vs []SchemaValue) {
	for _, v := range vs {
		ReleaseAll(v)
	}
}

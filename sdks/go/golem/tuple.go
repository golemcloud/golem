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
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// Tuples are positional, unnamed groups, lowering to the WIT tuple type. The
// types live in the shared core module; see [values.Tuple2].

// Tuple2 is a 2-element tuple.
type Tuple2[A, B any] = values.Tuple2[A, B]

// Tuple3 is a 3-element tuple.
type Tuple3[A, B, C any] = values.Tuple3[A, B, C]

// Tuple4 is a 4-element tuple.
type Tuple4[A, B, C, D any] = values.Tuple4[A, B, C, D]

// Tuple5 is a 5-element tuple.
type Tuple5[A, B, C, D, E any] = values.Tuple5[A, B, C, D, E]

// Tuple6 is a 6-element tuple.
type Tuple6[A, B, C, D, E, F any] = values.Tuple6[A, B, C, D, E, F]

// Tuple7 is a 7-element tuple.
type Tuple7[A, B, C, D, E, F, G any] = values.Tuple7[A, B, C, D, E, F, G]

// Tuple8 is an 8-element tuple.
type Tuple8[A, B, C, D, E, F, G, H any] = values.Tuple8[A, B, C, D, E, F, G, H]

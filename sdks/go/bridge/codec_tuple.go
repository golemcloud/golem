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

package bridge

import (
	"github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/core/values"
)

// Tuple conversions, one pair per arity the value vocabulary provides. They
// are identical but for the arity, and Go cannot abstract over it.

func EncodeTuple2[A, B any](t values.Tuple2[A, B], fa Encoder[A], fb Encoder[B]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B)}}
}

func DecodeTuple2[A, B any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B]) (values.Tuple2[A, B], error) {
	var out values.Tuple2[A, B]
	els, err := tupleElements(sv, 2)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	out.B, err = fb(els[1])
	return out, err
}

func EncodeTuple3[A, B, C any](t values.Tuple3[A, B, C], fa Encoder[A], fb Encoder[B], fc Encoder[C]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C)}}
}

func DecodeTuple3[A, B, C any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C]) (values.Tuple3[A, B, C], error) {
	var out values.Tuple3[A, B, C]
	els, err := tupleElements(sv, 3)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	out.C, err = fc(els[2])
	return out, err
}

func EncodeTuple4[A, B, C, D any](t values.Tuple4[A, B, C, D], fa Encoder[A], fb Encoder[B], fc Encoder[C], fd Encoder[D]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C), fd(t.D)}}
}

func DecodeTuple4[A, B, C, D any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C], fd Decoder[D]) (values.Tuple4[A, B, C, D], error) {
	var out values.Tuple4[A, B, C, D]
	els, err := tupleElements(sv, 4)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	if out.C, err = fc(els[2]); err != nil {
		return out, err
	}
	out.D, err = fd(els[3])
	return out, err
}

func EncodeTuple5[A, B, C, D, E any](t values.Tuple5[A, B, C, D, E], fa Encoder[A], fb Encoder[B], fc Encoder[C], fd Encoder[D], fe Encoder[E]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C), fd(t.D), fe(t.E)}}
}

func DecodeTuple5[A, B, C, D, E any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C], fd Decoder[D], fe Decoder[E]) (values.Tuple5[A, B, C, D, E], error) {
	var out values.Tuple5[A, B, C, D, E]
	els, err := tupleElements(sv, 5)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	if out.C, err = fc(els[2]); err != nil {
		return out, err
	}
	if out.D, err = fd(els[3]); err != nil {
		return out, err
	}
	out.E, err = fe(els[4])
	return out, err
}

func EncodeTuple6[A, B, C, D, E, F any](t values.Tuple6[A, B, C, D, E, F], fa Encoder[A], fb Encoder[B], fc Encoder[C], fd Encoder[D], fe Encoder[E], ff Encoder[F]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C), fd(t.D), fe(t.E), ff(t.F)}}
}

func DecodeTuple6[A, B, C, D, E, F any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C], fd Decoder[D], fe Decoder[E], ff Decoder[F]) (values.Tuple6[A, B, C, D, E, F], error) {
	var out values.Tuple6[A, B, C, D, E, F]
	els, err := tupleElements(sv, 6)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	if out.C, err = fc(els[2]); err != nil {
		return out, err
	}
	if out.D, err = fd(els[3]); err != nil {
		return out, err
	}
	if out.E, err = fe(els[4]); err != nil {
		return out, err
	}
	out.F, err = ff(els[5])
	return out, err
}

func EncodeTuple7[A, B, C, D, E, F, G any](t values.Tuple7[A, B, C, D, E, F, G], fa Encoder[A], fb Encoder[B], fc Encoder[C], fd Encoder[D], fe Encoder[E], ff Encoder[F], fg Encoder[G]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C), fd(t.D), fe(t.E), ff(t.F), fg(t.G)}}
}

func DecodeTuple7[A, B, C, D, E, F, G any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C], fd Decoder[D], fe Decoder[E], ff Decoder[F], fg Decoder[G]) (values.Tuple7[A, B, C, D, E, F, G], error) {
	var out values.Tuple7[A, B, C, D, E, F, G]
	els, err := tupleElements(sv, 7)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	if out.C, err = fc(els[2]); err != nil {
		return out, err
	}
	if out.D, err = fd(els[3]); err != nil {
		return out, err
	}
	if out.E, err = fe(els[4]); err != nil {
		return out, err
	}
	if out.F, err = ff(els[5]); err != nil {
		return out, err
	}
	out.G, err = fg(els[6])
	return out, err
}

func EncodeTuple8[A, B, C, D, E, F, G, H any](t values.Tuple8[A, B, C, D, E, F, G, H], fa Encoder[A], fb Encoder[B], fc Encoder[C], fd Encoder[D], fe Encoder[E], ff Encoder[F], fg Encoder[G], fh Encoder[H]) schema.SchemaValue {
	return schema.TupleValue{Elements: []schema.SchemaValue{fa(t.A), fb(t.B), fc(t.C), fd(t.D), fe(t.E), ff(t.F), fg(t.G), fh(t.H)}}
}

func DecodeTuple8[A, B, C, D, E, F, G, H any](sv schema.SchemaValue, fa Decoder[A], fb Decoder[B], fc Decoder[C], fd Decoder[D], fe Decoder[E], ff Decoder[F], fg Decoder[G], fh Decoder[H]) (values.Tuple8[A, B, C, D, E, F, G, H], error) {
	var out values.Tuple8[A, B, C, D, E, F, G, H]
	els, err := tupleElements(sv, 8)
	if err != nil {
		return out, err
	}
	if out.A, err = fa(els[0]); err != nil {
		return out, err
	}
	if out.B, err = fb(els[1]); err != nil {
		return out, err
	}
	if out.C, err = fc(els[2]); err != nil {
		return out, err
	}
	if out.D, err = fd(els[3]); err != nil {
		return out, err
	}
	if out.E, err = fe(els[4]); err != nil {
		return out, err
	}
	if out.F, err = ff(els[5]); err != nil {
		return out, err
	}
	if out.G, err = fg(els[6]); err != nil {
		return out, err
	}
	out.H, err = fh(els[7])
	return out, err
}

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

//go:build wasip1

package postgres

import (
	"fmt"

	pg "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_rdbms_postgres"
)

func init() { decodeLazyValue = decodeLazy }

func decode(v pg.DbValue) any {
	if x, ok := decodeFlat(v); ok {
		return x
	}
	return decodeLazy(v)
}

// decodeLazy handles the recursive families (array, composite, domain, custom
// range). It calls the host to read each lazy child and drops it afterwards.
func decodeLazy(v pg.DbValue) any {
	switch v.Tag() {
	case pg.DbValueArray:
		lazies := v.Array()
		out := make([]any, len(lazies))
		for i, l := range lazies {
			out[i] = decode(l.Get())
			l.Drop()
		}
		return out
	case pg.DbValueComposite:
		c := v.Composite()
		fields := make([]any, len(c.Values))
		for i, l := range c.Values {
			fields[i] = decode(l.Get())
			l.Drop()
		}
		return CompositeValue{Name: c.Name, Fields: fields}
	case pg.DbValueDomain:
		d := v.Domain()
		val := decode(d.Value.Get())
		d.Value.Drop()
		return DomainValue{Name: d.Name, Value: val}
	case pg.DbValueRange:
		rg := v.Range()
		return RangeValue{Name: rg.Name, Start: valueBoundFromWit(rg.Value.Start), End: valueBoundFromWit(rg.Value.End)}
	default:
		panic(fmt.Sprintf("golem/rdbms/postgres: unhandled value case %s", tagName(v.Tag())))
	}
}

func valueBoundFromWit(b pg.ValueBound) Bound[any] {
	switch b.Tag() {
	case pg.ValueBoundIncluded:
		l := b.Included()
		val := decode(l.Get())
		l.Drop()
		return Included[any](val)
	case pg.ValueBoundExcluded:
		l := b.Excluded()
		val := decode(l.Get())
		l.Drop()
		return Excluded[any](val)
	default:
		return Unbounded[any]()
	}
}

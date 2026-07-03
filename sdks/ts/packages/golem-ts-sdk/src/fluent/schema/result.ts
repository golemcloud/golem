// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Shared support for the WIT `result<ok, err>` type in the fluent walkers. A
// method whose `returns` schema is `s.result(ok, err)` lowers to a WIT
// `result-type`; the handler returns the SDK `Result<Ok, Err>` (`Result.ok(v)` /
// `Result.err(e)`) and the caller receives the decoded `Result<Ok, Err>`.
//
// This mirrors how the decorator ("normal") TS SDK maps a `Result<T, E>` return
// value to a component-model `result<S, E>` living INSIDE the success payload:
// the failure is a value, not the WIT `agent-error` channel (that stays reserved
// for throws / host traps).

import { Result } from '../../host/result';
import { FluentCodec } from './codec';
import { mergeGraphDefs, SchemaValue, t, v } from '../../internal/schema-model';

/**
 * Build a WIT `result<ok, err>` codec from the compiled ok / err member codecs.
 * `toValue` branches on the `Result` tag → `v.ok` / `v.err`; `fromValue` branches
 * on the decoded `SchemaResult` → `Result.ok` / `Result.err`. An absent (`void`)
 * arm payload round-trips as `undefined`.
 */
export function buildResultCodec(okCodec: FluentCodec, errCodec: FluentCodec): FluentCodec {
  const defs = mergeGraphDefs([okCodec.graph, errCodec.graph]);
  return {
    graph: { defs, root: t.result(okCodec.graph.root, errCodec.graph.root) },
    toValue: (value) => {
      const r = value as Result<unknown, unknown>;
      return r.tag === 'ok' ? v.ok(okCodec.toValue(r.val)) : v.err(errCodec.toValue(r.val));
    },
    fromValue: (sv) => {
      const rv = (sv as Extract<SchemaValue, { tag: 'result' }>).result;
      return rv.tag === 'ok'
        ? Result.ok(rv.value === undefined ? undefined : okCodec.fromValue(rv.value))
        : Result.err(rv.value === undefined ? undefined : errCodec.fromValue(rv.value));
    },
  };
}

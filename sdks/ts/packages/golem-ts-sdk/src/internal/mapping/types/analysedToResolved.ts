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

// TEMPORARY (removed in the agent-schema slice): lift the legacy analysis output
// (`AnalysedType`, produced by the existing `typeMapper`) into the new
// `ResolvedType` model. This lets Slice 2 establish the new schema model and its
// value codec without yet re-rooting the (still legacy) type-analysis handlers
// or the agent registration/runtime boundary. Once registration produces
// `ResolvedType` directly, this bridge and `AnalysedType` are deleted.

import { AnalysedType, CustomOrInbuilt, EmptyType, NameOptionTypePair } from './analysedType';
import {
  AbsentRepr,
  r,
  ResolvedType,
  ResolvedVariantCase,
  resolvedField,
  ResultRepr,
} from './resolvedType';

function emptyToAbsent(empty: EmptyType | undefined): AbsentRepr | undefined {
  if (empty === undefined) return undefined;
  return empty === 'null' ? 'null' : 'undefined';
}

function resultRepr(resultType: CustomOrInbuilt): ResultRepr {
  if (resultType.tag === 'inbuilt') {
    return {
      tag: 'inbuilt',
      okAbsent: emptyToAbsent(resultType.okEmptyType),
      errAbsent: emptyToAbsent(resultType.errEmptyType),
    };
  }
  return {
    tag: 'custom',
    okValueName: resultType.okValueName,
    errValueName: resultType.errValueName,
  };
}

/** Lift a legacy `AnalysedType` into the new `ResolvedType` model. */
export function analysedToResolved(typ: AnalysedType): ResolvedType {
  switch (typ.kind) {
    case 'bool':
      return r.bool();
    case 'string':
      return r.string();
    case 'chr':
      return r.char();
    case 'f64':
      return r.f64();
    case 'f32':
      return r.f32();
    case 'u64':
      return r.u64();
    case 's64':
      return r.s64();
    case 'u32':
      return r.u32();
    case 's32':
      return r.s32();
    case 'u16':
      return r.u16();
    case 's16':
      return r.s16();
    case 'u8':
      return r.u8();
    case 's8':
      return r.s8();

    case 'handle':
      throw new Error(
        'Resource handles are not supported by the schema model and cannot be mapped',
      );

    case 'option': {
      const { name, owner, inner } = typ.value;
      const noneRepr: AbsentRepr = typ.emptyType === 'null' ? 'null' : 'undefined';
      return r.option(analysedToResolved(inner), noneRepr, name, owner);
    }

    case 'list': {
      const { name, owner, inner } = typ.value;
      if (typ.mapType) {
        return r.map(
          analysedToResolved(typ.mapType.keyType),
          analysedToResolved(typ.mapType.valueType),
          name,
          owner,
        );
      }
      return r.list(analysedToResolved(inner), typ.typedArray, name, owner);
    }

    case 'tuple': {
      const { name, owner, items } = typ.value;
      return r.tuple(items.map(analysedToResolved), emptyToAbsent(typ.emptyType), name, owner);
    }

    case 'record': {
      const { name, owner, fields } = typ.value;
      return r.record(
        fields.map((f) => resolvedField(f.name, analysedToResolved(f.typ))),
        name,
        owner,
      );
    }

    case 'variant': {
      const { name, owner, cases } = typ.value;
      const tagged = typ.taggedTypes.length > 0;
      const resolvedCases: ResolvedVariantCase[] = cases.map((c: NameOptionTypePair) => {
        const payload = c.typ ? analysedToResolved(c.typ) : undefined;
        if (!tagged) {
          return { name: c.name, payload };
        }
        const metadata = typ.taggedTypes.find((m) => m.tagLiteralName === c.name);
        const valueKey = metadata?.valueType ? metadata.valueType[0] : undefined;
        return { name: c.name, payload, valueKey };
      });
      return r.variant(tagged, resolvedCases, name, owner);
    }

    case 'enum': {
      const { name, owner, cases } = typ.value;
      return r.enum(cases, name, owner);
    }

    case 'flags': {
      const { name, owner, names } = typ.value;
      return r.flags(names, name, owner);
    }

    case 'result': {
      const { name, owner, ok, err } = typ.value;
      return r.result(
        ok ? analysedToResolved(ok) : undefined,
        err ? analysedToResolved(err) : undefined,
        resultRepr(typ.resultType),
        name,
        owner,
      );
    }
  }
}

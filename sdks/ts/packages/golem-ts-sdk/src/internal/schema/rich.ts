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

// Schema-native rich types (mirrors the Rust SDK `agentic::{unstructured_text,
// unstructured_binary, multimodal}`). Unstructured text/binary project to a
// `variant { inline, url }`; multimodal projects to a `list<variant>` whose list
// node carries `metadata.role = multimodal`. Everything here operates only on
// the schema model (`SchemaType` / `SchemaValue`) and the SDK's `Unstructured*`
// value shapes — no legacy `DataValue` / `WitType` involved.

import { Role } from 'golem:core/types@2.0.0';
import {
  emptyMetadata,
  schemaType,
  SchemaType,
  SchemaValue,
  t,
  v,
  variantCase,
} from '../schema-model';
import { UnstructuredText } from '../../newTypes/textInput';
import { UnstructuredBinary } from '../../newTypes/binaryInput';

const MULTIMODAL_ROLE: Role = { tag: 'multimodal' };

// Variant case indices shared by unstructured text/binary.
const INLINE_CASE = 0;
const URL_CASE = 1;

// ============================================================
// Schema types
// ============================================================

export function unstructuredTextSchemaType(languages: string[]): SchemaType {
  const restrictions = languages.length > 0 ? { languages } : {};
  return t.variant([
    variantCase('inline', schemaType({ tag: 'text', restrictions })),
    variantCase('url', schemaType({ tag: 'url', restrictions: {} })),
  ]);
}

export function unstructuredBinarySchemaType(mimeTypes: string[]): SchemaType {
  const restrictions = mimeTypes.length > 0 ? { mimeTypes } : {};
  return t.variant([
    variantCase('inline', schemaType({ tag: 'binary', restrictions })),
    variantCase('url', schemaType({ tag: 'url', restrictions: {} })),
  ]);
}

export interface MultimodalCaseSchema {
  name: string;
  root: SchemaType;
}

/** `list<variant { … }>` with the list node tagged `role = multimodal`. */
export function multimodalSchemaType(cases: MultimodalCaseSchema[]): SchemaType {
  const list = t.list(t.variant(cases.map((c) => variantCase(c.name, c.root))));
  return { body: list.body, metadata: { ...emptyMetadata(), role: MULTIMODAL_ROLE } };
}

// ============================================================
// Value codecs — unstructured text
// ============================================================

export function unstructuredTextToValue(value: UnstructuredText): SchemaValue {
  if (value.tag === 'url') {
    return v.variant(URL_CASE, { tag: 'url', value: value.val });
  }
  return v.variant(INLINE_CASE, { tag: 'text', text: value.val, language: value.languageCode });
}

export function unstructuredTextFromValue(
  parameterName: string,
  value: SchemaValue,
  allowedCodes: string[],
): UnstructuredText<string[]> {
  if (value.tag !== 'variant') {
    throw new Error(
      `Expected variant value for unstructured-text parameter ${parameterName}, got ${value.tag}`,
    );
  }

  if (value.caseIndex === URL_CASE) {
    const payload = value.payload;
    if (!payload || payload.tag !== 'url') {
      throw new Error(`Expected url payload for unstructured-text parameter ${parameterName}`);
    }
    return { tag: 'url', val: payload.value };
  }

  if (value.caseIndex === INLINE_CASE) {
    const payload = value.payload;
    if (!payload || payload.tag !== 'text') {
      throw new Error(
        `Expected inline text payload for unstructured-text parameter ${parameterName}`,
      );
    }

    if (allowedCodes.length > 0) {
      if (!payload.language) {
        throw new Error(`Language code is required. Allowed codes: ${allowedCodes.join(', ')}`);
      }
      if (!allowedCodes.includes(payload.language)) {
        throw new Error(
          `Invalid value for parameter ${parameterName}. Language code \`${payload.language}\` is not allowed. Allowed codes: ${allowedCodes.join(', ')}`,
        );
      }
      return { tag: 'inline', val: payload.text, languageCode: payload.language };
    }

    return { tag: 'inline', val: payload.text };
  }

  throw new Error(
    `Unknown unstructured-text variant case ${value.caseIndex} for parameter ${parameterName}`,
  );
}

// ============================================================
// Value codecs — unstructured binary
// ============================================================

export function unstructuredBinaryToValue(value: UnstructuredBinary): SchemaValue {
  if (value.tag === 'url') {
    return v.variant(URL_CASE, { tag: 'url', value: value.val });
  }
  return v.variant(INLINE_CASE, { tag: 'binary', bytes: value.val, mimeType: value.mimeType });
}

export function unstructuredBinaryFromValue(
  parameterName: string,
  value: SchemaValue,
  allowedMimeTypes: string[],
): UnstructuredBinary {
  if (value.tag !== 'variant') {
    throw new Error(
      `Expected variant value for unstructured-binary parameter ${parameterName}, got ${value.tag}`,
    );
  }

  if (value.caseIndex === URL_CASE) {
    const payload = value.payload;
    if (!payload || payload.tag !== 'url') {
      throw new Error(`Expected url payload for unstructured-binary parameter ${parameterName}`);
    }
    return { tag: 'url', val: payload.value };
  }

  if (value.caseIndex === INLINE_CASE) {
    const payload = value.payload;
    if (!payload || payload.tag !== 'binary') {
      throw new Error(
        `Expected inline binary payload for unstructured-binary parameter ${parameterName}`,
      );
    }
    if (
      allowedMimeTypes.length > 0 &&
      (!payload.mimeType || !allowedMimeTypes.includes(payload.mimeType))
    ) {
      throw new Error(
        `Invalid value for parameter ${parameterName}. Mime type \`${payload.mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(', ')}`,
      );
    }
    return {
      tag: 'inline',
      val: payload.bytes,
      mimeType: payload.mimeType ?? '',
    } as UnstructuredBinary;
  }

  throw new Error(
    `Unknown unstructured-binary variant case ${value.caseIndex} for parameter ${parameterName}`,
  );
}

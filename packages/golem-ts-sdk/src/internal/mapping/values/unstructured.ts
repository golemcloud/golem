// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { BinaryReference, TextReference } from 'golem:agent/common';

import util from 'node:util';

import { Value } from './Value';

export function serializeBinaryReferenceToValue(tsValue: any): Value {
  const binaryReference = castTsValueToBinaryReference(tsValue);

  switch (binaryReference.tag) {
    case 'url':
      return {
        kind: 'variant',
        caseIdx: 0,
        caseValue: { kind: 'string', value: binaryReference.val },
      };

    case 'inline':
      return {
        kind: 'variant',
        caseIdx: 1,
        caseValue: {
          kind: 'record',
          value: [
            {
              kind: 'list',
              value: Array.from(binaryReference.val.data).map((b) => ({
                kind: 'u8',
                value: b,
              })),
            },
            {
              kind: 'record',
              value: [
                {
                  kind: 'string',
                  value: binaryReference.val.binaryType.mimeType,
                },
              ],
            },
          ],
        },
      };
  }
}

export function serializeTextReferenceToValue(tsValue: any): Value {
  const textReference: TextReference = castTsValueToTextReference(tsValue);

  switch (textReference.tag) {
    case 'url':
      return {
        kind: 'variant',
        caseIdx: 0,
        caseValue: { kind: 'string', value: textReference.val },
      };

    case 'inline':
      if (textReference.val.textType) {
        return {
          kind: 'variant',
          caseIdx: 1,
          caseValue: {
            kind: 'record',
            value: [
              { kind: 'string', value: textReference.val.data },
              {
                kind: 'option',
                value: {
                  kind: 'record',
                  value: [
                    {
                      kind: 'string',
                      value: textReference.val.textType.languageCode,
                    },
                  ],
                },
              },
            ],
          },
        };
      }

      return {
        kind: 'variant',
        caseIdx: 1,
        caseValue: {
          kind: 'record',
          value: [
            { kind: 'string', value: textReference.val.data },
            { kind: 'option' },
          ],
        },
      };
  }
}

export function castTsValueToBinaryReference(tsValue: any): BinaryReference {
  if (typeof tsValue === 'object') {
    const keys = Object.keys(tsValue);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(
          tsValue,
        )} to UnstructuredBinary. Missing 'tag' property.`,
      );
    }

    const tag = tsValue['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: tsValue['val'],
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            tsValue,
          )} to UnstructuredBinary. Missing 'val' property for tag 'url'.`,
        );
      }
    }

    if (typeof tag === 'string' && tag === 'inline') {
      if (keys.includes('val') && keys.includes('mimeType')) {
        return {
          tag: 'inline',
          val: {
            data: tsValue['val'],
            binaryType: {
              mimeType: tsValue['mimeType'],
            },
          },
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            tsValue,
          )} to UnstructuredBinary. Missing 'val' property for tag 'inline'.`,
        );
      }
    }

    throw new Error(
      `Unable to cast value ${util.format(
        tsValue,
      )} to UnstructuredBinary. Invalid 'tag' property: ${tag}. Expected 'url' or 'inline'.`,
    );
  }

  throw new Error(
    `Unable to cast value ${util.format(
      tsValue,
    )} to UnstructuredBinary. Expected an object with 'tag' and 'val' properties.`,
  );
}

export function castTsValueToTextReference(value: any): TextReference {
  if (typeof value === 'object') {
    const keys = Object.keys(value);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(
          value,
        )} to UnstructuredText. Missing 'tag' property.`,
      );
    }

    const tag = value['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: value['val'],
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredText. Missing 'val' property for tag 'url'.`,
        );
      }
    }

    if (typeof tag === 'string' && tag === 'inline') {
      if (keys.includes('val')) {
        if (keys.includes('languageCode')) {
          return {
            tag: 'inline',
            val: {
              data: value['val'],
              textType: {
                languageCode: value['languageCode'],
              },
            },
          };
        } else {
          return {
            tag: 'inline',
            val: {
              data: value['val'],
            },
          };
        }
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredText. Missing 'val' property for tag 'inline'.`,
        );
      }
    }

    throw new Error(
      `Unable to cast value ${util.format(
        value,
      )} to UnstructuredText. Invalid 'tag' property: ${tag}. Expected 'url' or 'inline'.`,
    );
  }

  throw new Error(
    `Unable to cast value ${util.format(
      value,
    )} to UnstructuredText. Expected an object with 'tag' and 'val' properties.`,
  );
}

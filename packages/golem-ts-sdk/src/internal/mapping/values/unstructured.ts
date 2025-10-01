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

import {
  BinaryReference,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import util from 'node:util';

export function serializeUnstructuredBinary(value: any): ElementValue {
  if (typeof value === 'object') {
    const keys = Object.keys(value);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(
          value,
        )} to UnstructuredBinary. Missing 'tag' property.`,
      );
    }

    const tag = value['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        const binaryReference: BinaryReference = {
          tag: 'url',
          val: value['val'],
        };

        return {
          tag: 'unstructured-binary',
          val: binaryReference,
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredBinary. Missing 'val' property for tag 'url'.`,
        );
      }
    }

    if (typeof tag === 'string' && tag === 'inline') {
      if (keys.includes('val') && keys.includes('mimeType')) {
        const binaryReference: BinaryReference = {
          tag: 'inline',
          val: {
            data: value['val'],
            binaryType: {
              mimeType: value['mimeType'],
            },
          },
        };

        return {
          tag: 'unstructured-binary',
          val: binaryReference,
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredBinary. Missing 'val' property for tag 'inline'.`,
        );
      }
    }

    throw new Error(
      `Unable to cast value ${util.format(
        value,
      )} to UnstructuredBinary. Invalid 'tag' property: ${tag}. Expected 'url' or 'inline'.`,
    );
  }

  throw new Error(
    `Unable to cast value ${util.format(
      value,
    )} to UnstructuredBinary. Expected an object with 'tag' and 'val' properties.`,
  );
}

export function serializeUnstructuredText(value: any): ElementValue {
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
        const textReference: TextReference = {
          tag: 'url',
          val: value['val'],
        };

        return {
          tag: 'unstructured-text',
          val: textReference,
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
          const textReference: TextReference = {
            tag: 'inline',
            val: {
              data: value['val'],
              textType: {
                languageCode: value['languageCode'],
              },
            },
          };

          return {
            tag: 'unstructured-text',
            val: textReference,
          };
        } else {
          const textReference: TextReference = {
            tag: 'inline',
            val: {
              data: value['val'],
            },
          };
          return {
            tag: 'unstructured-text',
            val: textReference,
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

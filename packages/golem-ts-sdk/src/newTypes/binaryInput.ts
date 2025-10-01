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

import { ElementSchema, BinaryReference } from 'golem:agent/common';

/**
 * Represents unstructured binary input, which can be either a URL or inline binary data.
 *
 * Example usage:
 *
 * ```ts
 * const urlBinary: UnstructuredBinary = UnstructuredBinary.fromUrl("https://example.com/file.bin");
 * const inlineBinary: UnstructuredBinary = UnstructuredBinary.fromInline(new
 *   Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream"
 * );
 * ```
 */
type MimeType = string;

export type UnstructuredBinary<MT extends MimeType[] = []> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: Uint8Array;
      mimeType?: MT[number];
    };

export const UnstructuredBinary = {
  fromDataValue<MT extends string[] = []>(
    dataValue: BinaryReference,
    allowedMimeTypes: string[],
  ): UnstructuredBinary<MT> {
    if (dataValue.tag === 'url') {
      return {
        tag: 'url',
        val: dataValue.val,
      };
    }

    if (allowedMimeTypes.length > 0) {
      if (!allowedMimeTypes.includes(dataValue.val.binaryType.mimeType)) {
        throw new Error(
          `Language code ${dataValue.val.binaryType.mimeType} is not allowed. Allowed codes: ${allowedMimeTypes.join(', ')}`,
        );
      }

      return {
        tag: 'inline',
        val: dataValue.val.data,
        mimeType: dataValue.val.binaryType.mimeType,
      };
    }

    return {
      tag: 'inline',
      val: dataValue.val.data,
    };
  },

  /**
   *
   * Creates a `UnstructuredBinary` from a URL.
   *
   * @param urlValue
   */
  fromUrl(urlValue: string): UnstructuredBinary {
    return {
      tag: 'url',
      val: urlValue,
    };
  },

  /**
   * Creates a `UnstructuredBinary` from inline binary data.
   *
   * @param data
   * @param mimeType
   */
  fromInline<MT extends MimeType[] = []>(
    data: Uint8Array,
    mimeType?: MT[number],
  ): UnstructuredBinary<MT> {
    return {
      tag: 'inline',
      val: data,
      mimeType: mimeType,
    };
  },
};

export const BinarySchema = {
  fromMimeType(mimeTypes?: string[]): ElementSchema {
    if (mimeTypes) {
      return {
        tag: 'unstructured-binary',
        val: {
          restrictions: mimeTypes.map((mimeType) => {
            return { mimeType: mimeType };
          }),
        },
      };
    }

    return {
      tag: 'unstructured-binary',
      val: {},
    };
  },
};

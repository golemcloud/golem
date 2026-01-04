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

import { BinaryReference } from 'golem:agent/common';
import * as Either from '../newTypes/either';

/**
 * Represents unstructured binary input, which can be either a URL or inline binary data.
 *
 * Example usage:
 *
 * ```ts
 * const inlineBinary: UnstructuredBinary<'application/json'> =
 *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
 *
 * const urlBinary: UnstructuredBinary =
 *   UnstructuredBinary.fromUrl("https://example.com/file.bin");
 *```
 *
 * If no mime types are specified, any mime type is allowed. Note that
 * when using `inline` you always need to pass a mime-type as we don't allow
 * unstructured-binary without mime type.
 *
 * ```ts
 *  function foo(input: UnstructuredBinary) {..} // any mime type allowed
 *  function bar(input: UnstructuredBinary<['application/json', 'image/png']>) {..} // only application/json and image/png allowed
 *
 *  const imageBinary: UnstructuredBinary =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "image/jpeg");
 *
 *  const textBinary: UnstructuredBinary<'text/plain'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "text/plain");
 *
 *  foo(imageBinary); // allowed
 *  foo(textBinary); // allowed
 *
 *  bar(imageBinary); // not allowed
 *
 *  const appJsonBinary: UnstructuredBinary<'application/json'> =
 *    UnstructuredBinary.fromInline(Uint8Array([0x00]), "application/json");
 *
 *  bar(appJsonBinary); // allowed
 *
 * ```
 */
type MimeType = string;

export type UnstructuredBinary<MT extends MimeType[] | MimeType = MimeType> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: Uint8Array;
      mimeType: MT extends MimeType[] ? MT[number] : MimeType;
    };

export const UnstructuredBinary = {
  fromDataValue<MT extends string[] | MimeType = MimeType>(
    parameterName: string,
    dataValue: BinaryReference,
    allowedMimeTypes: string[],
  ): Either.Either<UnstructuredBinary<MT>, string> {
    if (dataValue.tag === 'url') {
      return Either.right({
        tag: 'url',
        val: dataValue.val,
      } as UnstructuredBinary<MT>);
    }

    if (
      allowedMimeTypes.length > 0 &&
      !allowedMimeTypes.includes(dataValue.val.binaryType.mimeType)
    ) {
      return Either.left(
        `Invalid value for parameter ${parameterName}. Mime type \`${dataValue.val.binaryType.mimeType}\` is not allowed. Allowed mime types: ${allowedMimeTypes.join(', ')}`,
      );
    }

    return Either.right({
      tag: 'inline',
      val: dataValue.val.data,
      mimeType: dataValue.val.binaryType.mimeType,
    } as UnstructuredBinary<MT>);
  },

  /**
   *
   * Creates a `UnstructuredBinary` from a URL.
   *
   * Example usage:
   *
   * ```ts
   *
   * const urlBinary: UnstructuredBinary =
   *   UnstructuredBinary.fromUrl("https://example.com/file.bin");
   *
   * ```
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
   * Example usage:
   *
   * ```ts
   *
   * const inlineBinary: UnstructuredBinary<'application/json'> =
   *   UnstructuredBinary.fromInline(Uint8Array([0x00, 0x01, 0x02]), "application/octet-stream");
   *
   * ```
   *
   * @param data
   * @param mimeType
   */
  fromInline<MT extends MimeType[] | MimeType = MimeType>(
    data: Uint8Array,
    mimeType: MT extends MimeType[] ? MT[number] : MimeType,
  ): UnstructuredBinary<MT> {
    return {
      tag: 'inline',
      val: data,
      mimeType: mimeType,
    };
  },
};

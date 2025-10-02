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

import { TextReference } from 'golem:agent/common';
import * as Either from '../newTypes/either';
/**
 * Represents unstructured text input, which can be either a URL or inline text.
 *
 * Example usage:
 *
 * ```ts
 * const urlText: UnstructuredText = UnstructuredText.fromUrl("https://example.com");
 * const inlineText: UnstructuredText = UnstructuredText.fromInline("Hello, world!", ['en']);
 * ```
 */
type LanguageCode = string;

export type UnstructuredText<LC extends LanguageCode[] = []> =
  | {
      tag: 'url';
      val: string;
    }
  | {
      tag: 'inline';
      val: string;
      languageCode?: LC[number];
    };

export const UnstructuredText = {
  fromDataValue<LC extends string[] = []>(
    parameterName: string,
    dataValue: TextReference,
    allowedCodes: string[],
  ): Either.Either<UnstructuredText<LC>, string> {
    if (dataValue.tag === 'url') {
      return Either.right({
        tag: 'url',
        val: dataValue.val,
      });
    }

    if (allowedCodes.length > 0) {
      if (!dataValue.val.textType) {
        return Either.left(
          `Language code is required. Allowed codes: ${allowedCodes.join(', ')}`,
        );
      }

      if (!allowedCodes.includes(dataValue.val.textType.languageCode)) {
        return Either.left(
          `Invalid value for parameter ${parameterName}. Language code \`${dataValue.val.textType.languageCode}\` is not allowed. Allowed codes: ${allowedCodes.join(', ')}`,
        );
      }

      return Either.right({
        tag: 'inline',
        val: dataValue.val.data,
        languageCode: dataValue.val.textType.languageCode,
      });
    }

    return Either.right({
      tag: 'inline',
      val: dataValue.val.data,
    });
  },

  /**
   * Creates a `UnstructuredText` from a URL.
   *
   * @param urlValue
   *
   */
  fromUrl(urlValue: string): UnstructuredText {
    return {
      tag: 'url',
      val: urlValue,
    };
  },

  /**
   * Creates a `TextInput` with a default language code of `'en'`.
   *
   * @param data
   * @param languageCode - The language code
   * @returns A `TextInput` object with `languageCode` set to `'en'`.
   */
  fromInline<LC extends LanguageCode[] = []>(
    data: string,
    languageCode?: LC[number],
  ): UnstructuredText<LC> {
    return {
      tag: 'inline',
      val: data,
      languageCode: languageCode,
    };
  },
};

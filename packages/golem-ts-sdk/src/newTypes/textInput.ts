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

import { ElementSchema, TextReference } from 'golem:agent/common';

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
  languageCode?: LC;
};

export const UnstructuredText = {
  fromDataValue<LC extends string[] = []>(dataValue: TextReference, allowedCodes: string[]): UnstructuredText<LC> {

    if (dataValue.tag === 'url') {
      return {
        tag: 'url',
        val: dataValue.val,
      };
    }

    if (allowedCodes.length > 0) {
      if (!dataValue.val.textType) {
        throw new Error(`Language code is required. Allowed codes: ${allowedCodes.join(', ')}`);
      }

      if (!allowedCodes.includes(dataValue.val.textType.languageCode)) {
        throw new Error(`Language code ${dataValue.val.textType.languageCode} is not allowed. Allowed codes: ${allowedCodes.join(', ')}`);
      }
    }

    return {
      tag: 'inline',
      val: dataValue.val.data,
      languageCode: allowedCodes as unknown as LC,
    };
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
  fromInline<LC extends LanguageCode[] = []>(data: string, languageCode?: LC): UnstructuredText<LC> {
    return {
      tag: 'inline',
      val: data,
      languageCode: languageCode,
    };
  },
};

export const TextSchema = {
  fromLanguageCode(languageCodes?: string[]): ElementSchema {
    if (languageCodes) {
      return {
        tag: 'unstructured-text',
        val: {
          restrictions: languageCodes.map((code) => {
            return { languageCode: code };
          }),
        },
      };
    }

    return {
      tag: 'unstructured-text',
      val: {},
    };
  },
};


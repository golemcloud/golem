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

export type TextType = {
  languageCode: string;
};

export type TextSource = {
  data: string;
  textType: TextType;
};

export type UnstructuredText =
  | { tag: 'url'; val: string }
  | { tag: 'inline'; val: TextSource };

export const TextInput = {
  fromTextReferenceDataValue(dataValue: TextReference): UnstructuredText {
    if (dataValue.tag === 'url') {
      return { tag: 'url', val: dataValue.val };
    }

    return {
      tag: 'inline',
      val: {
        data: dataValue.val.data,
        textType: {
          languageCode: dataValue.val.textType?.languageCode ?? 'en',
        },
      },
    };
  },

  /**
   * Creates a `UnstructuredText` from a URL.
   *
   * @param urlValue
   *
   */
  fromUrl(urlValue: string): UnstructuredText {
    return { tag: 'url', val: urlValue };
  },

  /**
   * Creates a `TextInput` with a default language code of `'en'`.
   *
   * @param data
   * @param languageCode - The language code
   * @returns A `TextInput` object with `languageCode` set to `'en'`.
   */
  fromInline(data: string, languageCode?: string): UnstructuredText {
    languageCode = languageCode ? languageCode : 'en';
    return { tag: 'inline', val: { data: data, textType: { languageCode } } };
  },
};

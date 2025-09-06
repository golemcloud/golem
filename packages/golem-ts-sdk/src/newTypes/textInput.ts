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
  /**
   * Creates a `TextInput` with a default language code of `'en'`.
   *
   * @param input - The text content.
   * @param languageCode - The language code
   * @returns A `TextInput` object with `languageCode` set to `'en'`.
   */
  fromText(input: string, languageCode?: string): UnstructuredText {
    languageCode = languageCode ? languageCode : 'en';

    return { tag: 'inline', val: { data: input, textType: { languageCode } } };
  },

  /**
   * Creates a `TextInput` from a URL.
   *
   * @param urlValue
   */
  fromUrl(urlValue: string): UnstructuredText {
    return { tag: 'url', val: urlValue };
  },
};

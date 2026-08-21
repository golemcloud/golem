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

import { describe, it, expect } from 'vitest';
import { route } from '../src/route';

describe('route()', () => {
  describe('absolute paths (no baseUrl)', () => {
    it('substitutes a single path variable', () => {
      expect(route('/assets/{assetName}', { assetName: 'logo.png' })).toBe('/assets/logo.png');
    });

    it('renders literal-only templates unchanged', () => {
      expect(route('/health', {})).toBe('/health');
    });

    it('substitutes multiple path variables', () => {
      expect(route('/a/{x}/b/{y}', { x: '1', y: '2' })).toBe('/a/1/b/2');
    });

    it('percent-encodes substituted values', () => {
      expect(route('/files/{name}', { name: 'a b/c?d' })).toBe('/files/a%20b%2Fc%3Fd');
    });

    it('coerces numeric and boolean args', () => {
      expect(route('/items/{id}/flag/{on}', { id: 42, on: true })).toBe('/items/42/flag/true');
    });

    it('keeps slashes for a remaining-path variable', () => {
      expect(route('/static/{*rest}', { rest: 'img/icons/logo.svg' })).toBe(
        '/static/img/icons/logo.svg',
      );
    });

    it('appends inline query parameters', () => {
      expect(route('/search?q={term}&page={page}', { term: 'hello world', page: 2 })).toBe(
        '/search?q=hello%20world&page=2',
      );
    });

    it('reuses one arg when a variable appears in both path and query', () => {
      expect(route('/users/{name}?u={name}', { name: 'amy' })).toBe('/users/amy?u=amy');
    });

    it('encodes query parameter names and values', () => {
      expect(route('/lookup?a b={v}', { v: 'x&y' })).toBe('/lookup?a%20b=x%26y');
    });
  });

  describe('fully qualified URLs (baseUrl)', () => {
    it('prefixes a bare host with https by default', () => {
      expect(
        route('/assets/{assetName}', { assetName: 'logo.png' }, { baseUrl: 'mygolemhost.com' }),
      ).toBe('https://mygolemhost.com/assets/logo.png');
    });

    it('honours the http scheme flag', () => {
      expect(
        route(
          '/assets/{assetName}',
          { assetName: 'logo.png' },
          { baseUrl: 'mygolemhost.com', scheme: 'http' },
        ),
      ).toBe('http://mygolemhost.com/assets/logo.png');
    });

    it('keeps the scheme embedded in baseUrl', () => {
      expect(route('/x', {}, { baseUrl: 'http://example.com', scheme: 'https' })).toBe(
        'http://example.com/x',
      );
    });

    it('strips trailing slashes from baseUrl', () => {
      expect(route('/x', {}, { baseUrl: 'example.com///' })).toBe('https://example.com/x');
    });

    it('includes the query string in qualified URLs', () => {
      expect(route('/add?by={by}', { by: 5 }, { baseUrl: 'example.com' })).toBe(
        'https://example.com/add?by=5',
      );
    });
  });

  describe('validation', () => {
    it('throws listing missing path variables', () => {
      expect(() => route('/a/{x}/b/{y}', { x: '1' })).toThrow(
        'Missing value(s) for route variable(s): "y"',
      );
    });

    it('throws listing missing query variables', () => {
      expect(() => route('/search?q={term}', {})).toThrow('"term"');
    });

    it('throws on unknown arguments', () => {
      expect(() => route('/a/{x}', { x: '1', extra: 'nope' })).toThrow(
        'Unknown route argument "extra"',
      );
    });

    it('rejects templates that do not start with "/"', () => {
      expect(() => route('assets/{name}', { name: 'x' })).toThrow();
    });

    it('rejects invalid query segments', () => {
      expect(() => route('/search?q={term}&oops', { term: 'x' })).toThrow();
    });

    it('rejects empty string templates', () => {
      expect(() => route('', {})).toThrow();
    });
  });
});

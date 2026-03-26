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
import { encodeMultipart, decodeMultipart, MultipartPart } from '../src/internal/multipart';

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function jsonPart(name: string, value: unknown): MultipartPart {
  return {
    name,
    contentType: 'application/json',
    body: textEncoder.encode(JSON.stringify(value)),
  };
}

function binaryPart(name: string, bytes: Uint8Array): MultipartPart {
  return {
    name,
    contentType: 'application/octet-stream',
    body: bytes,
  };
}

describe('multipart encode/decode', () => {
  it('round-trip: encode then decode produces same parts', () => {
    const parts: MultipartPart[] = [
      jsonPart('metadata', { key: 'value' }),
      binaryPart('payload', new Uint8Array([1, 2, 3, 4, 5])),
    ];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(2);
    for (let i = 0; i < parts.length; i++) {
      expect(decoded[i].name).toBe(parts[i].name);
      expect(decoded[i].contentType).toBe(parts[i].contentType);
      expect(decoded[i].body).toEqual(parts[i].body);
    }
  });

  it('single JSON part', () => {
    const obj = { hello: 'world', count: 42 };
    const parts: MultipartPart[] = [jsonPart('data', obj)];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(1);
    expect(decoded[0].name).toBe('data');
    expect(decoded[0].contentType).toBe('application/json');
    expect(JSON.parse(textDecoder.decode(decoded[0].body))).toEqual(obj);
  });

  it('multiple parts with mixed content types', () => {
    const parts: MultipartPart[] = [
      jsonPart('json-part', { a: 1 }),
      {
        name: 'text-part',
        contentType: 'text/plain',
        body: textEncoder.encode('hello world'),
      },
      binaryPart('bin-part', new Uint8Array([0xff, 0xfe, 0xfd])),
    ];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(3);
    expect(decoded[0].contentType).toBe('application/json');
    expect(decoded[1].contentType).toBe('text/plain');
    expect(decoded[2].contentType).toBe('application/octet-stream');
  });

  it('binary content with null bytes and boundary-like patterns', () => {
    // Build bytes that include null bytes and text that looks like a boundary marker
    const fakeBoundary = textEncoder.encode('\r\n--somefakeboundary\r\n');
    const nullHeavy = new Uint8Array(64);
    for (let i = 0; i < nullHeavy.length; i++) {
      nullHeavy[i] = i % 3 === 0 ? 0x00 : i;
    }

    const combined = new Uint8Array(fakeBoundary.length + nullHeavy.length);
    combined.set(fakeBoundary, 0);
    combined.set(nullHeavy, fakeBoundary.length);

    const parts: MultipartPart[] = [binaryPart('sqlite-blob', combined)];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(1);
    expect(decoded[0].body).toEqual(combined);
  });

  it('empty body part', () => {
    const parts: MultipartPart[] = [
      {
        name: 'empty',
        contentType: 'application/octet-stream',
        body: new Uint8Array(0),
      },
    ];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(1);
    expect(decoded[0].name).toBe('empty');
    expect(decoded[0].body).toEqual(new Uint8Array(0));
  });

  it('part names are preserved', () => {
    const names = ['alpha', 'beta-2', 'gamma_3'];
    const parts: MultipartPart[] = names.map((n) => jsonPart(n, {}));

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded.map((p) => p.name)).toEqual(names);
  });

  it('content types are preserved', () => {
    const contentTypes = ['application/json', 'text/html; charset=utf-8', 'image/png'];
    const parts: MultipartPart[] = contentTypes.map((ct, i) => ({
      name: `part-${i}`,
      contentType: ct,
      body: new Uint8Array([i]),
    }));

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded.map((p) => p.contentType)).toEqual(contentTypes);
  });

  it('decode rejects duplicate part names', () => {
    const parts: MultipartPart[] = [
      jsonPart('dup', { first: true }),
      jsonPart('dup', { second: true }),
    ];

    const { data, boundary } = encodeMultipart(parts);

    expect(() => decodeMultipart(data, boundary)).toThrow('Duplicate multipart part name: dup');
  });

  it('decode rejects missing boundary', () => {
    const parts: MultipartPart[] = [jsonPart('x', {})];
    const { data } = encodeMultipart(parts);

    expect(() => decodeMultipart(data, 'wrongboundary')).toThrow(
      'Multipart body does not start with boundary',
    );
  });

  it('boundary does not collide with binary content', () => {
    // Create a part whose body contains many boundary-like strings
    const lines: string[] = [];
    for (let i = 0; i < 50; i++) {
      lines.push(`\r\n--${crypto.randomUUID().replace(/-/g, '')}`);
    }
    const body = textEncoder.encode(lines.join(''));
    const parts: MultipartPart[] = [binaryPart('tricky', body)];

    const { data, boundary } = encodeMultipart(parts);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(1);
    expect(decoded[0].body).toEqual(body);
  });

  it('CRLF encoding: encoded output uses \\r\\n', () => {
    const parts: MultipartPart[] = [jsonPart('check', { v: 1 })];
    const { data } = encodeMultipart(parts);
    const text = textDecoder.decode(data);

    // Every newline in the multipart framing must be \r\n
    const lines = text.split('\r\n');
    expect(lines.length).toBeGreaterThan(1);

    // No bare \n should appear in the framing (outside the JSON body)
    const withoutBody = text.replace(JSON.stringify({ v: 1 }), '');
    expect(withoutBody).not.toMatch(/[^\r]\n/);
  });

  it('decode tolerates bare \\n (LF-only)', () => {
    // Manually build a multipart message that uses CRLF for the delimiter
    // boundaries but bare LF within headers (mixed line endings).
    // The decoder splits on \r\n--boundary, so we keep that for delimiters
    // but use bare \n for header line breaks inside each part.
    const boundary = 'testboundary123';
    const crlf = '\r\n';
    const lf = '\n';
    const raw =
      `--${boundary}${crlf}` +
      `Content-Type: application/json${lf}` +
      `Content-Disposition: attachment; name="lf-part"${lf}` +
      `${lf}` +
      `{"lf":true}${crlf}` +
      `--${boundary}--${crlf}`;

    const data = textEncoder.encode(raw);
    const decoded = decodeMultipart(data, boundary);

    expect(decoded).toHaveLength(1);
    expect(decoded[0].name).toBe('lf-part');
    expect(decoded[0].contentType).toBe('application/json');
    expect(JSON.parse(textDecoder.decode(decoded[0].body))).toEqual({
      lf: true,
    });
  });
});

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

import { describe, it, expect, vi } from 'vitest';
import fixtures from '../../../../../test-data/snapshot-multipart/framing.json';
import {
  encodeMultipart,
  decodeMultipart,
  extractBoundary,
  MultipartPart,
} from '../src/internal/multipart';

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
  const boundary = 'b';
  const header =
    'Content-Type: application/octet-stream\r\nContent-Disposition: attachment; name="part:index"\r\n';
  const wire = `--b\r\n${header}\r\nX\r\n--b--\r\n`;
  for (const bad of [
    wire.slice(0, -7),
    wire + 'epilogue',
    wire + '\r\n',
    'preamble' + wire,
    '\r\n\r\n' + wire,
    wire.replace('--b\r\n', '--bextra\r\n'),
    wire.replace('--b--\r\n', '--b--extra\r\n'),
    wire.replace(header, header + 'content-type: text/plain\r\n'),
    wire.replace(header, header + 'CONTENT-DISPOSITION: attachment; name="other"\r\n'),
    wire.replace('name="part:index"', 'name="part:index"; name="other"'),
    wire.replace(header, header + 'Content-Transfer-Encoding: base64\r\n'),
    wire.replace('application/octet-stream', 'application/\roctet-stream'),
  ]) {
    it(`rejects malformed framing/header ${JSON.stringify(bad)}`, () => {
      expect(() => decodeMultipart(textEncoder.encode(bad), boundary)).toThrow();
    });
  }
  it('accepts exactly one leading framing newline and closing EOF', () => {
    expect(
      textDecoder.decode(
        decodeMultipart(textEncoder.encode('\r\n' + wire.slice(0, -2)), boundary)[0].body,
      ),
    ).toBe('X');
  });
  it('round-trips every byte, including 0 and 255', () => {
    const body = Uint8Array.from({ length: 256 }, (_, i) => i);
    const encoded = encodeMultipart([binaryPart('part:__proto__', body)]);
    expect(decodeMultipart(encoded.data, encoded.boundary)[0].body).toEqual(body);
  });
  it('preserves a large binary sandwich independently of the input buffer', () => {
    const body = Uint8Array.from({ length: 4 * 1024 * 1024 }, (_, i) => i % 256);
    const parts = [
      jsonPart('state', { count: 17 }),
      binaryPart('db:primary', body),
      jsonPart('tail', { last: true }),
    ];
    const encoded = encodeMultipart(parts);
    const decoded = decodeMultipart(encoded.data, encoded.boundary);
    expect(decoded.map(({ name, contentType }) => ({ name, contentType }))).toEqual(
      parts.map(({ name, contentType }) => ({ name, contentType })),
    );
    for (const clearInput of [false, true]) {
      if (clearInput) encoded.data.fill(0);
      for (let i = 0; i < parts.length; i++) {
        expect(Buffer.from(decoded[i].body).equals(Buffer.from(parts[i].body))).toBe(true);
      }
    }
  });
  for (const newline of ['\r\n', '\n']) {
    it(`preserves false delimiters and payload endings with ${JSON.stringify(newline)} framing`, () => {
      const body = `\n\n--almost\n--b\r\ninvalid\r\n--bextra\r\n--b--extra\r\n\r\nend\n`;
      // Bare LF before a complete marker is payload only in CRLF framing.
      const payload = newline === '\r\n' ? body : body.replace('\n--b\r\n', '\n--bextra\r\n');
      const data = textEncoder.encode(
        `--b${newline}Content-Type: application/octet-stream${newline}Content-Disposition: attachment; name="part:index"${newline}${newline}${payload}${newline}--b--`,
      );
      expect(textDecoder.decode(decodeMultipart(data, 'b')[0].body)).toBe(payload);
    });
  }
  for (const suffix of ['', '\r', 'x', '--', '--\r', '--x']) {
    it(`classifies final-position collisions after false candidates: ${JSON.stringify(suffix)}`, () => {
      const first = '11111111111111111111111111111111';
      const second = '22222222222222222222222222222222';
      const random = vi
        .spyOn(crypto, 'randomUUID')
        .mockReturnValueOnce('11111111-1111-1111-1111-111111111111')
        .mockReturnValue('22222222-2222-2222-2222-222222222222');
      try {
        const body = textEncoder.encode(`\r\r\n--almost\r\n--${first}x\r\n--${first}${suffix}`);
        const encoded = encodeMultipart([binaryPart('part:index', body)]);
        expect(encoded.boundary).toBe(suffix === '' || suffix === '--' ? second : first);
        expect(decodeMultipart(encoded.data, encoded.boundary)[0].body).toEqual(body);
      } finally {
        random.mockRestore();
      }
    });
  }
  for (const payload of [
    '--B\r\ninside',
    'inside\r\n--B\r\nend',
    'inside\r\n--B',
    '--B--',
    'inside\r\n--B--',
  ]) {
    it(`regenerates a forced collision ${JSON.stringify(payload)}`, () => {
      const first = '11111111111111111111111111111111';
      const second = '22222222222222222222222222222222';
      const random = vi
        .spyOn(crypto, 'randomUUID')
        .mockReturnValueOnce('11111111-1111-1111-1111-111111111111')
        .mockReturnValue('22222222-2222-2222-2222-222222222222');
      try {
        const body = textEncoder.encode(payload.replace(/B/g, first));
        const encoded = encodeMultipart([binaryPart('part:index', body)]);
        expect(encoded.boundary).toBe(second);
        expect(decodeMultipart(encoded.data, encoded.boundary)[0].body).toEqual(body);
      } finally {
        random.mockRestore();
      }
    });
  }
  it('rejects header injection while encoding', () => {
    expect(() =>
      encodeMultipart([binaryPart('bad"\r\nInjected: yes', new Uint8Array())]),
    ).toThrow();
    expect(() =>
      encodeMultipart([
        { name: 'part:index', contentType: 'text/plain\r\nInjected: yes', body: new Uint8Array() },
      ]),
    ).toThrow();
  });
  it('rejects lossy names and malformed UTF-8 headers', () => {
    for (const name of ['db:\uD800', 'db:\uDC00', 'db:\u0000', 'db:\u007f']) {
      expect(() => encodeMultipart([binaryPart(name, new Uint8Array())])).toThrow();
    }
    const raw = textEncoder.encode(wire);
    const position = wire.indexOf('part:index');
    for (const invalid of [[255], [192, 175], [237, 160, 128]]) {
      const malformed = raw.slice();
      malformed.set(invalid, position);
      expect(() => decodeMultipart(malformed, boundary)).toThrow('invalid UTF-8 header');
    }
  });
  for (const mime of [
    'multipart/mixedextra; boundary=b',
    'multipart/mixed; boundary=b; boundary=b',
    'multipart/mixed; boundary="b',
    'multipart/mixed; boundary=',
    'multipart/mixed; boundary="b c"',
    'multipart/mixed; boundary=' + 'b'.repeat(71),
  ]) {
    it(`rejects invalid boundary MIME ${mime}`, () => expect(extractBoundary(mime)).toBeNull());
  }

  for (const fixture of fixtures.valid) {
    it(`shared framing: ${fixture.name}`, () => {
      const e = fixture.newline;
      const data = textEncoder.encode(
        `--${fixtures.boundary}${e}Content-Type: application/octet-stream${e}Content-Disposition: attachment; name="part:index"${e}${e}${fixture.payload}${e}--${fixtures.boundary}--${e}`,
      );
      expect(Array.from(decodeMultipart(data, fixtures.boundary)[0].body)).toEqual(
        fixture.hex.match(/../g)?.map((b) => parseInt(b, 16)) ?? [],
      );
    });
  }

  it('requires a closing delimiter', () => {
    const raw =
      '--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name="state"\r\n\r\n{}';
    expect(() => decodeMultipart(textEncoder.encode(raw), 'b')).toThrow();
  });

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

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

const CRLF = '\r\n';

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export interface MultipartPart {
  name: string;
  contentType: string;
  body: Uint8Array;
}

function generateBoundary(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID().replace(/-/g, '');
  }
  let hex = '';
  for (let i = 0; i < 32; i++) {
    hex += Math.floor(Math.random() * 16).toString(16);
  }
  return hex;
}

function containsBoundary(parts: MultipartPart[], boundary: string): boolean {
  const marker = textEncoder.encode(`${CRLF}--${boundary}`);
  for (const part of parts) {
    if (indexOf(part.body, marker) !== -1) {
      return true;
    }
  }
  return false;
}

function indexOf(haystack: Uint8Array, needle: Uint8Array): number {
  if (needle.length === 0) return 0;
  if (needle.length > haystack.length) return -1;
  outer: for (let i = 0; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer;
    }
    return i;
  }
  return -1;
}

function concat(chunks: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const c of chunks) total += c.length;
  const result = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    result.set(c, offset);
    offset += c.length;
  }
  return result;
}

export function encodeMultipart(parts: MultipartPart[]): {
  data: Uint8Array;
  boundary: string;
} {
  let boundary = generateBoundary();
  while (containsBoundary(parts, boundary)) {
    boundary = generateBoundary();
  }

  const chunks: Uint8Array[] = [];

  for (const part of parts) {
    const header =
      `--${boundary}${CRLF}` +
      `Content-Type: ${part.contentType}${CRLF}` +
      `Content-Disposition: attachment; name="${part.name}"${CRLF}` +
      CRLF;
    chunks.push(textEncoder.encode(header));
    chunks.push(part.body);
    chunks.push(textEncoder.encode(CRLF));
  }

  chunks.push(textEncoder.encode(`--${boundary}--${CRLF}`));

  return { data: concat(chunks), boundary };
}

export function decodeMultipart(data: Uint8Array, boundary: string): MultipartPart[] {
  const delimiter = textEncoder.encode(`${CRLF}--${boundary}`);
  const crlfBytes = textEncoder.encode(CRLF);
  const lfByte = 0x0a; // \n

  // Normalise: strip leading CRLF or LF if present
  let start = 0;
  if (data.length >= 2 && data[0] === 0x0d && data[1] === 0x0a) {
    start = 2;
  } else if (data.length >= 1 && data[0] === 0x0a) {
    start = 1;
  }

  // The first boundary appears without a leading CRLF
  const firstDelimiter = textEncoder.encode(`--${boundary}`);

  // Verify the body starts with the opening boundary
  if (!startsWith(data, firstDelimiter, start)) {
    throw new Error('Multipart body does not start with boundary');
  }

  // Split the body on the delimiter
  const rawSections = splitOnDelimiter(data, delimiter, start + firstDelimiter.length);

  const parts: MultipartPart[] = [];
  const seenNames = new Set<string>();

  for (const section of rawSections) {
    // Skip the closing delimiter section
    if (startsWith(section, textEncoder.encode('--'), 0)) {
      continue;
    }

    // Section starts after delimiter; skip the CRLF or LF that follows it
    let sectionStart = 0;
    if (section.length >= 2 && section[0] === 0x0d && section[1] === 0x0a) {
      sectionStart = 2;
    } else if (section.length >= 1 && section[0] === 0x0a) {
      sectionStart = 1;
    }

    // Find the blank line separating headers from body (CRLF CRLF or LF LF)
    const headerEndCrlf = findDoubleNewline(section, sectionStart, crlfBytes);
    const headerEndLf = findDoubleNewlineLf(section, sectionStart);

    let headerEnd: number;
    let bodyStart: number;

    if (headerEndCrlf !== -1 && (headerEndLf === -1 || headerEndCrlf <= headerEndLf)) {
      headerEnd = headerEndCrlf;
      bodyStart = headerEndCrlf + 4; // skip \r\n\r\n
    } else if (headerEndLf !== -1) {
      headerEnd = headerEndLf;
      bodyStart = headerEndLf + 2; // skip \n\n
    } else {
      throw new Error('Could not find end of headers in multipart part');
    }

    const headerText = textDecoder.decode(section.slice(sectionStart, headerEnd));
    const headerLines = headerText.split(/\r?\n/);

    let contentType = '';
    let name = '';

    for (const line of headerLines) {
      const colonIdx = line.indexOf(':');
      if (colonIdx === -1) continue;
      const key = line.slice(0, colonIdx).trim().toLowerCase();
      const value = line.slice(colonIdx + 1).trim();

      if (key === 'content-type') {
        contentType = value;
      } else if (key === 'content-disposition') {
        const nameMatch = value.match(/name="([^"]+)"/);
        if (nameMatch) {
          name = nameMatch[1];
        }
      }
    }

    if (!name) {
      throw new Error('Multipart part missing name in Content-Disposition');
    }
    if (seenNames.has(name)) {
      throw new Error(`Duplicate multipart part name: ${name}`);
    }
    seenNames.add(name);

    // Body: everything after headers until end of section.
    // Strip trailing CRLF or LF that precedes the next delimiter.
    let bodyEnd = section.length;
    if (bodyEnd >= 2 && section[bodyEnd - 2] === 0x0d && section[bodyEnd - 1] === 0x0a) {
      bodyEnd -= 2;
    } else if (bodyEnd >= 1 && section[bodyEnd - 1] === lfByte) {
      bodyEnd -= 1;
    }

    const body = section.slice(bodyStart, bodyEnd);

    parts.push({ name, contentType, body });
  }

  return parts;
}

function startsWith(data: Uint8Array, prefix: Uint8Array, offset: number): boolean {
  if (offset + prefix.length > data.length) return false;
  for (let i = 0; i < prefix.length; i++) {
    if (data[offset + i] !== prefix[i]) return false;
  }
  return true;
}

function splitOnDelimiter(data: Uint8Array, delimiter: Uint8Array, start: number): Uint8Array[] {
  const sections: Uint8Array[] = [];
  let pos = start;

  while (pos < data.length) {
    const next = indexOf(data.slice(pos), delimiter);
    if (next === -1) {
      sections.push(data.slice(pos));
      break;
    }
    sections.push(data.slice(pos, pos + next));
    pos = pos + next + delimiter.length;
  }

  return sections;
}

function findDoubleNewline(data: Uint8Array, start: number, crlf: Uint8Array): number {
  // Look for \r\n\r\n
  for (let i = start; i <= data.length - 4; i++) {
    if (
      data[i] === crlf[0] &&
      data[i + 1] === crlf[1] &&
      data[i + 2] === crlf[0] &&
      data[i + 3] === crlf[1]
    ) {
      return i;
    }
  }
  return -1;
}

function findDoubleNewlineLf(data: Uint8Array, start: number): number {
  // Look for \n\n
  for (let i = start; i <= data.length - 2; i++) {
    if (data[i] === 0x0a && data[i + 1] === 0x0a) {
      return i;
    }
  }
  return -1;
}

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

import { blake3 } from '@noble/hashes/blake3';
import type {
  DiscriminatorRule,
  MetadataEnvelope,
  NumericBound,
  QuantityValue,
  SchemaGraph,
  SchemaType,
  TypeId,
} from './model';
import { validateSchemaGraph } from './validation';

const DOMAIN = 'golem-schema-fingerprint';

export class SchemaFingerprintError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SchemaFingerprintError';
  }
}

/** Compute the BLAKE3-256 SchemaFingerprintV1 digest for a stream element schema closure. */
export function schemaFingerprintV1(graph: SchemaGraph, element?: SchemaType): Uint8Array {
  return blake3(canonicalSchemaBytesV1(graph, element));
}

/** The normative deterministic-CBOR input to {@link schemaFingerprintV1}. */
export function canonicalSchemaBytesV1(graph: SchemaGraph, element?: SchemaType): Uint8Array {
  const root = element ?? { body: { tag: 'tuple', elements: [] }, metadata: emptyMetadata() };
  const definitions = reachableDefinitions(graph, root).sort(([left], [right]) =>
    compareUtf8(left, right),
  );
  const projected: SchemaGraph = { defs: new Map(definitions), root };
  const errors = validateSchemaGraph(projected);
  if (errors.length > 0) {
    throw new SchemaFingerprintError(
      `invalid stream element schema: ${errors.map((error) => error.message).join('; ')}`,
    );
  }

  const encoder = new CborEncoder();
  encoder.array(4);
  encoder.text(DOMAIN);
  encoder.unsigned(1n);
  if (element === undefined) {
    encoder.array(2);
    encoder.unsigned(0n);
    encodeMetadata(encoder, emptyMetadata());
  } else {
    encodeType(encoder, root);
  }
  encoder.array(definitions.length);
  for (const [id, definition] of definitions) {
    encoder.array(3);
    encoder.text(id);
    encoder.optionalText(definition.name);
    encodeType(encoder, definition.body);
  }
  return encoder.finish();
}

function reachableDefinitions(graph: SchemaGraph, root: SchemaType) {
  const found = new Map<
    TypeId,
    typeof graph.defs extends ReadonlyMap<TypeId, infer V> ? V : never
  >();
  const visiting = new Set<TypeId>();
  const visit = (type: SchemaType): void => {
    const body = type.body;
    if (body.tag === 'ref') {
      if (visiting.has(body.id)) return;
      const definition = graph.defs.get(body.id);
      if (definition === undefined) return;
      visiting.add(body.id);
      found.set(body.id, definition);
      visit(definition.body);
      return;
    }
    switch (body.tag) {
      case 'record':
        body.fields.forEach((field) => visit(field.body));
        break;
      case 'variant':
        body.cases.forEach((entry) => entry.payload && visit(entry.payload));
        break;
      case 'tuple':
        body.elements.forEach(visit);
        break;
      case 'list':
      case 'fixed-list':
      case 'option':
        visit(body.element);
        break;
      case 'map':
        visit(body.key);
        visit(body.value);
        break;
      case 'result':
        if (body.ok) visit(body.ok);
        if (body.err) visit(body.err);
        break;
      case 'union':
        body.branches.forEach((branch) => visit(branch.body));
        break;
      case 'secret':
        visit(body.inner);
        break;
      case 'future':
      case 'stream':
        if (body.element) visit(body.element);
        break;
    }
  };
  visit(root);
  return [...found.entries()];
}

function encodeType(encoder: CborEncoder, type: SchemaType): void {
  const body = type.body;
  const leaf = (tag: number): void => {
    encoder.array(2);
    encoder.unsigned(BigInt(tag));
    encodeMetadata(encoder, type.metadata);
  };
  const unary = (tag: number, element: SchemaType): void => {
    encoder.array(3);
    encoder.unsigned(BigInt(tag));
    encodeType(encoder, element);
    encodeMetadata(encoder, type.metadata);
  };
  switch (body.tag) {
    case 'ref':
      encoder.array(3);
      encoder.unsigned(1n);
      encoder.text(body.id);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'bool':
      leaf(2);
      break;
    case 's8':
      encodeNumeric(encoder, 3, body.restrictions, type.metadata);
      break;
    case 's16':
      encodeNumeric(encoder, 4, body.restrictions, type.metadata);
      break;
    case 's32':
      encodeNumeric(encoder, 5, body.restrictions, type.metadata);
      break;
    case 's64':
      encodeNumeric(encoder, 6, body.restrictions, type.metadata);
      break;
    case 'u8':
      encodeNumeric(encoder, 7, body.restrictions, type.metadata);
      break;
    case 'u16':
      encodeNumeric(encoder, 8, body.restrictions, type.metadata);
      break;
    case 'u32':
      encodeNumeric(encoder, 9, body.restrictions, type.metadata);
      break;
    case 'u64':
      encodeNumeric(encoder, 10, body.restrictions, type.metadata);
      break;
    case 'f32':
      encodeNumeric(encoder, 11, body.restrictions, type.metadata);
      break;
    case 'f64':
      encodeNumeric(encoder, 12, body.restrictions, type.metadata);
      break;
    case 'char':
      leaf(13);
      break;
    case 'string':
      leaf(14);
      break;
    case 'record':
      encoder.array(3);
      encoder.unsigned(15n);
      encoder.array(body.fields.length);
      body.fields.forEach((field) => {
        encoder.array(3);
        encoder.text(field.name);
        encodeType(encoder, field.body);
        encodeMetadata(encoder, field.metadata);
      });
      encodeMetadata(encoder, type.metadata);
      break;
    case 'variant':
      encoder.array(3);
      encoder.unsigned(16n);
      encoder.array(body.cases.length);
      body.cases.forEach((entry) => {
        encoder.array(3);
        encoder.text(entry.name);
        encodeOptionalType(encoder, entry.payload);
        encodeMetadata(encoder, entry.metadata);
      });
      encodeMetadata(encoder, type.metadata);
      break;
    case 'enum':
      encodeNames(encoder, 17, body.cases, type.metadata);
      break;
    case 'flags':
      encodeNames(encoder, 18, body.names, type.metadata);
      break;
    case 'tuple':
      encoder.array(3);
      encoder.unsigned(19n);
      encoder.array(body.elements.length);
      body.elements.forEach((element) => encodeType(encoder, element));
      encodeMetadata(encoder, type.metadata);
      break;
    case 'list':
      unary(20, body.element);
      break;
    case 'fixed-list':
      encoder.array(4);
      encoder.unsigned(21n);
      encodeType(encoder, body.element);
      encoder.unsigned(BigInt(body.length));
      encodeMetadata(encoder, type.metadata);
      break;
    case 'map':
      encoder.array(4);
      encoder.unsigned(22n);
      encodeType(encoder, body.key);
      encodeType(encoder, body.value);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'option':
      unary(23, body.element);
      break;
    case 'result':
      encoder.array(4);
      encoder.unsigned(24n);
      encodeOptionalType(encoder, body.ok);
      encodeOptionalType(encoder, body.err);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'text':
      encoder.array(6);
      encoder.unsigned(25n);
      encodeOptionalSet(encoder, 'text.languages', body.restrictions.languages);
      encoder.optionalUnsigned(body.restrictions.minLength);
      encoder.optionalUnsigned(body.restrictions.maxLength);
      encoder.optionalText(body.restrictions.regex);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'binary':
      encoder.array(5);
      encoder.unsigned(26n);
      encodeOptionalSet(encoder, 'binary.mime_types', body.restrictions.mimeTypes);
      encoder.optionalUnsigned(body.restrictions.minBytes);
      encoder.optionalUnsigned(body.restrictions.maxBytes);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'path':
      encoder.array(6);
      encoder.unsigned(27n);
      encoder.unsigned(BigInt(['input', 'output', 'in-out'].indexOf(body.spec.direction)));
      encoder.unsigned(BigInt(['file', 'directory', 'any'].indexOf(body.spec.kind)));
      encodeOptionalSet(encoder, 'path.allowed_mime_types', body.spec.allowedMimeTypes);
      encodeOptionalSet(encoder, 'path.allowed_extensions', body.spec.allowedExtensions);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'url':
      encoder.array(4);
      encoder.unsigned(28n);
      encodeOptionalSet(encoder, 'url.allowed_schemes', body.restrictions.allowedSchemes);
      encodeOptionalSet(encoder, 'url.allowed_hosts', body.restrictions.allowedHosts);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'datetime':
      leaf(29);
      break;
    case 'duration':
      leaf(30);
      break;
    case 'quantity':
      encoder.array(6);
      encoder.unsigned(31n);
      encoder.text(body.spec.baseUnit);
      encoder.array(body.spec.allowedSuffixes.length);
      body.spec.allowedSuffixes.forEach((suffix) => encoder.text(suffix));
      encodeOptionalQuantity(encoder, body.spec.min);
      encodeOptionalQuantity(encoder, body.spec.max);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'union':
      encoder.array(3);
      encoder.unsigned(32n);
      encoder.array(body.branches.length);
      body.branches.forEach((branch) => {
        encoder.array(4);
        encoder.text(branch.tag);
        encodeType(encoder, branch.body);
        encodeDiscriminator(encoder, branch.discriminator);
        encodeMetadata(encoder, branch.metadata);
      });
      encodeMetadata(encoder, type.metadata);
      break;
    case 'secret':
      encoder.array(4);
      encoder.unsigned(33n);
      encodeType(encoder, body.inner);
      encoder.optionalText(body.spec.category);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'quota-token':
      encoder.array(3);
      encoder.unsigned(34n);
      encoder.optionalText(body.spec.resourceName);
      encodeMetadata(encoder, type.metadata);
      break;
    case 'future':
    case 'stream':
      encoder.array(3);
      encoder.unsigned(body.tag === 'future' ? 35n : 36n);
      encodeOptionalType(encoder, body.element);
      encodeMetadata(encoder, type.metadata);
      break;
  }
}

function encodeNumeric(
  encoder: CborEncoder,
  tag: number,
  restrictions: { min?: NumericBound; max?: NumericBound; unit?: string } | undefined,
  metadata: MetadataEnvelope,
): void {
  encoder.array(3);
  encoder.unsigned(BigInt(tag));
  const normalized =
    restrictions &&
    (restrictions.min !== undefined ||
      restrictions.max !== undefined ||
      (restrictions.unit ?? '') !== '');
  if (!normalized) encoder.null();
  else {
    encoder.array(3);
    encodeOptionalBound(encoder, restrictions.min);
    encodeOptionalBound(encoder, restrictions.max);
    encoder.optionalText(restrictions.unit === '' ? undefined : restrictions.unit);
  }
  encodeMetadata(encoder, metadata);
}

function encodeOptionalBound(encoder: CborEncoder, bound?: NumericBound): void {
  if (!bound) return encoder.null();
  encoder.array(2);
  encoder.unsigned(BigInt(bound.tag === 'signed' ? 0 : bound.tag === 'unsigned' ? 1 : 2));
  if (bound.tag === 'signed') encoder.signed(bound.val);
  else if (bound.tag === 'unsigned') encoder.unsigned(bound.val);
  else encoder.unsigned(floatIsZero(bound.val) ? 0n : bound.val);
}

function floatIsZero(bits: bigint): boolean {
  return (bits & 0x7fffffffffffffffn) === 0n;
}

function encodeNames(
  encoder: CborEncoder,
  tag: number,
  names: string[],
  metadata: MetadataEnvelope,
): void {
  encoder.array(3);
  encoder.unsigned(BigInt(tag));
  encoder.array(names.length);
  names.forEach((name) => encoder.text(name));
  encodeMetadata(encoder, metadata);
}

function encodeOptionalType(encoder: CborEncoder, type?: SchemaType): void {
  if (type) encodeType(encoder, type);
  else encoder.null();
}

function encodeOptionalQuantity(encoder: CborEncoder, value?: QuantityValue): void {
  if (!value) return encoder.null();
  encoder.array(3);
  encoder.signed(value.mantissa);
  encoder.signed(BigInt(value.scale));
  encoder.text(value.unit);
}

function encodeDiscriminator(encoder: CborEncoder, rule: DiscriminatorRule): void {
  const simple = { prefix: 0, suffix: 1, contains: 2, regex: 3 } as const;
  if (rule.tag in simple) {
    encoder.array(2);
    encoder.unsigned(BigInt(simple[rule.tag as keyof typeof simple]));
    encoder.text(rule.val as string);
  } else if (rule.tag === 'field-equals') {
    encoder.array(3);
    encoder.unsigned(4n);
    encoder.text(rule.val.fieldName);
    encoder.optionalText(rule.val.literal);
  } else {
    encoder.array(2);
    encoder.unsigned(5n);
    encoder.text((rule as Extract<DiscriminatorRule, { tag: 'field-absent' }>).val);
  }
}

function encodeMetadata(encoder: CborEncoder, metadata: MetadataEnvelope): void {
  encoder.array(5);
  encoder.optionalText(metadata.doc);
  encodeSet(encoder, 'metadata.aliases', metadata.aliases);
  encoder.array(metadata.examples.length);
  metadata.examples.forEach((example) => encoder.text(example));
  encoder.optionalText(metadata.deprecated);
  if (!metadata.role) encoder.null();
  else if (metadata.role.tag === 'other') {
    encoder.array(2);
    encoder.unsigned(3n);
    encoder.text(metadata.role.val);
  } else {
    encoder.array(1);
    encoder.unsigned(
      BigInt(
        metadata.role.tag === 'multimodal' ? 0 : metadata.role.tag === 'unstructured-text' ? 1 : 2,
      ),
    );
  }
}

function encodeOptionalSet(encoder: CborEncoder, field: string, values?: string[]): void {
  if (!values) encoder.null();
  else encodeSet(encoder, field, values);
}
function encodeSet(encoder: CborEncoder, field: string, values: string[]): void {
  const sorted = [...values].sort(compareUtf8);
  for (let index = 1; index < sorted.length; index++)
    if (sorted[index - 1] === sorted[index])
      throw new SchemaFingerprintError(
        `duplicate value \`${sorted[index]}\` in set-valued schema field \`${field}\``,
      );
  encoder.array(sorted.length);
  sorted.forEach((value) => encoder.text(value));
}

function emptyMetadata(): MetadataEnvelope {
  return { aliases: [], examples: [] };
}
const textEncoder = new TextEncoder();
function utf8(value: string): Uint8Array {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      if (++i >= value.length || value.charCodeAt(i) < 0xdc00 || value.charCodeAt(i) > 0xdfff)
        throw new SchemaFingerprintError('schema contains invalid UTF-8 text');
    } else if (code >= 0xdc00 && code <= 0xdfff)
      throw new SchemaFingerprintError('schema contains invalid UTF-8 text');
  }
  return textEncoder.encode(value);
}
function compareUtf8(left: string, right: string): number {
  const a = utf8(left),
    b = utf8(right);
  const length = Math.min(a.length, b.length);
  for (let i = 0; i < length; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return a.length - b.length;
}

class CborEncoder {
  private readonly bytes: number[] = [];
  finish(): Uint8Array {
    return Uint8Array.from(this.bytes);
  }
  unsigned(value: bigint): void {
    if (value < 0n || value > 0xffffffffffffffffn)
      throw new SchemaFingerprintError('CBOR integer is outside u64 range');
    this.major(0, value);
  }
  signed(value: bigint): void {
    if (value >= 0n) this.unsigned(value);
    else this.major(1, -1n - value);
  }
  array(length: number): void {
    this.major(4, BigInt(length));
  }
  text(value: string): void {
    const bytes = utf8(value);
    this.major(3, BigInt(bytes.length));
    this.bytes.push(...bytes);
  }
  optionalText(value?: string): void {
    if (value === undefined) this.null();
    else this.text(value);
  }
  optionalUnsigned(value?: number): void {
    if (value === undefined) this.null();
    else this.unsigned(BigInt(value));
  }
  null(): void {
    this.bytes.push(0xf6);
  }
  private major(major: number, value: bigint): void {
    const prefix = major << 5;
    if (value <= 23n) this.bytes.push(prefix | Number(value));
    else if (value <= 0xffn) this.bytes.push(prefix | 24, Number(value));
    else if (value <= 0xffffn) this.pushInteger(prefix | 25, value, 2);
    else if (value <= 0xffffffffn) this.pushInteger(prefix | 26, value, 4);
    else this.pushInteger(prefix | 27, value, 8);
  }
  private pushInteger(initial: number, value: bigint, width: number): void {
    this.bytes.push(initial);
    for (let shift = width - 1; shift >= 0; shift--)
      this.bytes.push(Number((value >> BigInt(shift * 8)) & 0xffn));
  }
}

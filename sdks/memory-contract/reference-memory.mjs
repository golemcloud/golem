// A test-only reference Memory and the three composition forms. check.test.mjs runs the
// semantic fixtures through them to check that the fixtures are consistent with the contract.
// Nothing here is a public API, a backend or an SDK implementation.

import { randomUUID } from 'node:crypto';

export class MemoryError extends Error {
  constructor(kind, message, { member, limit, cause } = {}) {
    super(message, cause === undefined ? undefined : { cause });
    this.kind = kind;
    this.member = member;
    this.limit = limit;
  }
}

// Native cancellation. It is not a MemoryError, so no Memory converts it into one.
export class Cancelled extends Error {}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export const isObservationId = (value) => typeof value === 'string' && UUID.test(value);

const words = (text) => text.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);

function matches(query, text) {
  const wanted = new Set(words(query));
  return words(text).some((word) => wanted.has(word));
}

const utf8Bytes = (text) => Buffer.byteLength(text, 'utf8');

export const render = (items) => items.map((item) => item.text).join('\n');

export function pack(candidates, tokenBudget, measureTokens) {
  const chosen = [];
  for (const candidate of candidates) {
    if (measureTokens(render([...chosen, candidate])) <= tokenBudget) chosen.push(candidate);
  }
  return { items: chosen, tokens: measureTokens(render(chosen)) };
}

// The candidate allowance of one operation. A composite passes one allowance to all members.
export class Allowance {
  constructor(limits) {
    this.rows = limits.candidateRows;
    this.bytes = limits.candidateBytes;
  }

  take(text) {
    const bytes = utf8Bytes(text);
    if (this.rows < 1) throw new MemoryError('limit', 'candidate rows exhausted', { limit: 'candidateRows' });
    if (bytes > this.bytes) throw new MemoryError('limit', 'candidate bytes exhausted', { limit: 'candidateBytes' });
    this.rows -= 1;
    this.bytes -= bytes;
  }
}

export function validateSchema(schema, value) {
  switch (schema.type) {
    case 'string':
      return typeof value === 'string' && value.length >= (schema.minLength ?? 0);
    case 'array':
      return (
        Array.isArray(value) &&
        value.length >= (schema.minItems ?? 0) &&
        value.every((element) => validateSchema(schema.items, element))
      );
    case 'object': {
      if (value === null || typeof value !== 'object' || Array.isArray(value)) return false;
      const properties = schema.properties ?? {};
      if ((schema.required ?? []).some((key) => !Object.hasOwn(value, key))) return false;
      return Object.entries(value).every(([key, field]) =>
        Object.hasOwn(properties, key) ? validateSchema(properties[key], field) : schema.additionalProperties !== false,
      );
    }
    default:
      throw new Error(`unsupported schema type ${schema.type}`);
  }
}

function checkLimit(name, value) {
  if (!Number.isInteger(value) || value < 0) throw new MemoryError('input', `${name} must be a nonnegative integer`);
}

function checkSelectors(input) {
  const byIds = input.observationIds !== undefined;
  const byReferences = input.references !== undefined;
  if (byIds === byReferences) throw new MemoryError('input', 'forget needs exactly one of observationIds and references');
  const list = byIds ? input.observationIds : input.references;
  if (!Array.isArray(list) || list.length === 0) throw new MemoryError('input', 'forget needs a nonempty selector list');
  if (byIds && !list.every(isObservationId)) throw new MemoryError('input', 'observation IDs are UUIDs');
  return byIds;
}

class Base {
  tools(input = {}) {
    const namespace = input.namespace ?? 'memory';
    if (typeof namespace !== 'string' || namespace === '') throw new MemoryError('input', 'namespace is a nonempty string');
    return {
      namespace,
      tools: [
        { name: `${namespace}.recall`, role: 'recall', invoke: (args, ctx) => this.recall(args, ctx) },
        { name: `${namespace}.note`, role: 'note', invoke: (args, ctx) => this.observe({ parts: [args.text] }, ctx) },
        { name: `${namespace}.forget`, role: 'forget', invoke: (args, ctx) => this.forget(args, ctx) },
      ],
    };
  }
}

export function assemble(toolsets) {
  const names = new Set();
  for (const toolset of toolsets) {
    for (const tool of toolset.tools) {
      if (names.has(tool.name)) throw new MemoryError('input', `duplicate tool name ${tool.name}`);
      names.add(tool.name);
    }
  }
  return toolsets.flatMap((toolset) => toolset.tools);
}

// A custom memory. It keeps sources, one summary derivation per source and preset items, the
// fixed knowledge of a scripted member. Retrieval is word matching in insertion order.
export class ReferenceMemory extends Base {
  constructor(name, config, env) {
    super();
    this.name = name;
    this.limits = env.limits;
    this.summarizer = config.summarizer ? env.models.summarizer : undefined;
    this.queryEmbedder = config.queryEmbedder ? env.models.embedder : undefined;
    this.summarySchema = env.summarySchema;
    this.trace = env.trace;
    this.traced = false;
    this.faults = (config.faults ?? []).map((fault) => ({ ...fault }));
    this.calls = {};
    this.nextKey = 0;
    this.sources = new Map();
    this.derived = [];
    this.items = (config.items ?? []).map((item) => ({ key: this.nextKey++, text: item.text, sources: [...item.sources] }));
  }

  async observe(input, ctx) {
    return this.#record('observe', async () => {
      const parts = input.parts;
      const refs = input.refs ?? [];
      if (!Array.isArray(parts) || parts.length === 0 || !parts.every((part) => typeof part === 'string')) {
        throw new MemoryError('input', 'parts is a nonempty list of text');
      }
      if (input.id !== undefined && !isObservationId(input.id)) throw new MemoryError('input', 'observation IDs are UUIDs');
      const id = input.id ?? randomUUID();
      const existing = this.sources.get(id);
      if (existing) {
        if (JSON.stringify([existing.parts, existing.refs]) !== JSON.stringify([parts, refs])) {
          throw new MemoryError('input', `observation ${id} has different content`);
        }
      } else {
        this.sources.set(id, { id, parts: [...parts], refs: [...refs], caller: ctx.caller });
      }
      if (ctx.caller !== null) {
        const earlier = [...this.sources.values()].find(
          (source) => source.id !== id && source.caller === ctx.caller && this.#missing(source.id),
        );
        if (earlier) await this.#enrich(earlier.id, ctx);
      }
      if (this.#missing(id)) await this.#enrich(id, ctx);
      return id;
    });
  }

  async recall(input, ctx) {
    return this.#record('recall', async () => {
      checkLimit('limit', input.limit);
      if (input.limit === 0) return [];
      await this.#embedQuery(input.retrieval, ctx);
      return this.#candidates(input.query, ctx).slice(0, input.limit);
    });
  }

  async context(input, ctx) {
    return this.#record('context', async () => {
      checkLimit('tokenBudget', input.tokenBudget);
      if (typeof input.measureTokens !== 'function') throw new MemoryError('input', 'context needs measureTokens');
      if (input.tokenBudget === 0) return { items: [], tokens: 0 };
      await this.#embedQuery(input.retrieval, ctx);
      return pack(this.#candidates(input.query, ctx), input.tokenBudget, input.measureTokens);
    });
  }

  async forget(input, ctx) {
    return this.#record('forget', async () => {
      if (checkSelectors(input)) {
        const ids = new Set(input.observationIds);
        for (const id of ids) this.sources.delete(id);
        this.derived = this.derived.filter((entry) => !ids.has(entry.source));
        this.items = this.items
          .map((item) => ({ ...item, sources: item.sources.filter((source) => !ids.has(source)) }))
          .filter((item) => item.sources.length > 0);
        return null;
      }
      for (const reference of input.references) {
        if (reference?.owner !== this) throw new MemoryError('input', 'the reference belongs to another memory');
        if (reference.kind === 'source') throw new MemoryError('input', 'forget a source by its observation ID');
      }
      const keys = new Set(input.references.map((reference) => reference.key));
      this.items = this.items.filter((item) => !keys.has(item.key));
      this.derived = this.derived.filter((entry) => !keys.has(entry.key));
      return null;
    });
  }

  async #record(op, body) {
    const calls = (this.calls[op] = (this.calls[op] ?? 0) + 1);
    try {
      const fault = this.faults.find((entry) => entry.op === op && entry.nth === calls);
      if (fault) throw new MemoryError(fault.error, `injected ${fault.error} fault`);
      const result = await body();
      if (this.traced) this.trace.push({ event: 'member', member: this.name, op, outcome: 'ok' });
      return result;
    } catch (error) {
      const outcome = error instanceof Cancelled ? 'cancelled' : error.kind;
      if (this.traced) this.trace.push({ event: 'member', member: this.name, op, outcome });
      throw error;
    }
  }

  #missing(id) {
    return this.summarizer !== undefined && !this.derived.some((entry) => entry.source === id && entry.kind === 'summary');
  }

  async #enrich(id, ctx) {
    const source = this.sources.get(id);
    let output;
    try {
      output = await this.summarizer.call({ source: id, parts: source.parts, signal: ctx.signal });
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      this.trace.push({ event: 'diagnostic', member: this.name, source: id, role: 'summarizer', cause: 'error' });
      return;
    }
    if (!validateSchema(this.summarySchema, output)) {
      this.trace.push({ event: 'diagnostic', member: this.name, source: id, role: 'summarizer', cause: 'malformed' });
      return;
    }
    // The support recheck and the insert-if-absent run together, with no suspension between them.
    if (!this.sources.has(id) || !this.#missing(id)) return;
    this.derived.push({ key: this.nextKey++, source: id, kind: 'summary', text: output.summary });
  }

  async #embedQuery(retrieval, ctx) {
    const mode = retrieval ?? (this.queryEmbedder ? 'hybrid' : 'lexical');
    if (mode !== 'lexical' && mode !== 'hybrid') throw new MemoryError('input', `unknown retrieval ${mode}`);
    if (mode === 'lexical') return;
    if (!this.queryEmbedder) throw new MemoryError('input', 'hybrid retrieval needs an embedder');
    try {
      await this.queryEmbedder.call({ signal: ctx.signal });
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      throw new MemoryError('retrieval-model', 'the query embedding failed', { cause: error });
    }
  }

  #candidates(query, ctx) {
    const allowance = ctx.allowance ?? new Allowance(this.limits);
    const all = [
      ...this.items.map((item) => ({ text: item.text, sources: item.sources, ref: { owner: this, kind: 'item', key: item.key } })),
      ...[...this.sources.values()].map((source) => ({
        text: source.parts.join('\n'),
        sources: [source.id],
        ref: { owner: this, kind: 'source', key: source.id },
      })),
      ...this.derived.map((entry) => ({ text: entry.text, sources: [entry.source], ref: { owner: this, kind: 'derived', key: entry.key } })),
    ];
    const found = all.filter((candidate) => matches(query, candidate.text));
    for (const candidate of found) allowance.take(candidate.text);
    return found;
  }
}

function memberCount(members) {
  return members.reduce((count, [, memory]) => count + 1 + (memory instanceof Composite ? memory.memberCount : 0), 0);
}

function wrap(error, name) {
  if (!(error instanceof MemoryError)) throw error;
  throw new MemoryError(error.kind, `member ${name}: ${error.message}`, { member: name, limit: error.limit, cause: error });
}

export class Composite extends Base {
  constructor(form, members, { limits, classifier, candidateSchema }) {
    super();
    if (!['product', 'fallback', 'arbiter'].includes(form)) throw new MemoryError('input', `unknown form ${form}`);
    if (members.length === 0) throw new MemoryError('input', 'a composite needs members');
    const names = members.map(([name]) => name);
    if (new Set(names).size !== names.length) throw new MemoryError('input', 'member names are unique');
    this.memberCount = memberCount(members);
    if (this.memberCount > limits.memberFanOut) {
      throw new MemoryError('limit', 'too many members', { limit: 'memberFanOut' });
    }
    this.form = form;
    this.members = members;
    this.limits = limits;
    this.classifier = classifier;
    this.selectionSchema = candidateSchema;
  }

  async observe(input, ctx) {
    if (input.id !== undefined && !isObservationId(input.id)) throw new MemoryError('input', 'observation IDs are UUIDs');
    const id = input.id ?? randomUUID();
    for (const [name, memory] of this.members) {
      await memory.observe({ ...input, id }, ctx).catch((error) => wrap(error, name));
    }
    return id;
  }

  async recall(input, ctx) {
    checkLimit('limit', input.limit);
    if (input.limit === 0) return [];
    const memberCtx = { ...ctx, allowance: ctx.allowance ?? new Allowance(this.limits) };
    const read = (name, memory) => memory.recall(input, memberCtx).catch((error) => wrap(error, name));
    if (this.form === 'fallback') {
      for (const [name, memory] of this.members) {
        const items = await read(name, memory);
        if (items.length > 0) return this.#own(items, name);
      }
      return [];
    }
    const results = [];
    for (const [name, memory] of this.members) results.push([name, await read(name, memory)]);
    if (this.form === 'product') return this.#interleave(results).slice(0, input.limit);
    const chosen = await this.#choose(input.query, results, ctx);
    return chosen ? this.#own(chosen[1], chosen[0]) : [];
  }

  async context(input, ctx) {
    checkLimit('tokenBudget', input.tokenBudget);
    if (typeof input.measureTokens !== 'function') throw new MemoryError('input', 'context needs measureTokens');
    if (input.tokenBudget === 0) return { items: [], tokens: 0 };
    const memberCtx = { ...ctx, allowance: ctx.allowance ?? new Allowance(this.limits) };
    const read = (name, memory) => memory.context(input, memberCtx).catch((error) => wrap(error, name));
    if (this.form === 'fallback') {
      for (const [name, memory] of this.members) {
        const result = await read(name, memory);
        if (result.items.length > 0) return { items: this.#own(result.items, name), tokens: result.tokens };
      }
      return { items: [], tokens: 0 };
    }
    const results = [];
    for (const [name, memory] of this.members) results.push([name, await read(name, memory)]);
    if (this.form === 'product') {
      return pack(this.#interleave(results.map(([name, result]) => [name, result.items])), input.tokenBudget, input.measureTokens);
    }
    const chosen = await this.#choose(input.query, results.map(([name, result]) => [name, result.items, result]), ctx);
    return chosen ? { items: this.#own(chosen[1], chosen[0]), tokens: chosen[2].tokens } : { items: [], tokens: 0 };
  }

  async forget(input, ctx) {
    if (checkSelectors(input)) {
      for (const [name, memory] of this.members) {
        await memory.forget({ observationIds: input.observationIds }, ctx).catch((error) => wrap(error, name));
      }
      return null;
    }
    if (!input.references.every((reference) => reference?.owner === this)) {
      throw new MemoryError('input', 'the reference belongs to another memory');
    }
    const byName = new Map(this.members);
    for (const reference of input.references) {
      for (const [name, target] of reference.targets) {
        await byName.get(name).forget({ references: [target] }, ctx).catch((error) => wrap(error, name));
      }
    }
    return null;
  }

  #own(items, name) {
    return items.map((item) => ({ text: item.text, sources: item.sources, origins: [name], ref: { owner: this, targets: [[name, item.ref]] } }));
  }

  #interleave(results) {
    const owned = results.map(([name, items]) => this.#own(items, name));
    const merged = [];
    const byKey = new Map();
    for (let index = 0; owned.some((items) => index < items.length); index++) {
      for (const items of owned) {
        const item = items[index];
        if (!item) continue;
        const key = JSON.stringify([item.text, item.sources]);
        const existing = byKey.get(key);
        if (existing) {
          existing.origins.push(...item.origins);
          existing.ref.targets.push(...item.ref.targets);
        } else {
          byKey.set(key, item);
          merged.push(item);
        }
      }
    }
    return merged;
  }

  async #choose(query, results, ctx) {
    const nonempty = results.filter(([, items]) => items.length > 0);
    if (nonempty.length <= 1) return nonempty[0];
    if (!this.classifier) throw new MemoryError('input', 'an arbiter needs a classifier');
    let output;
    try {
      output = await this.classifier.call({
        signal: ctx.signal,
        query,
        candidates: nonempty.map(([member, items]) => ({ member, items: items.map((item) => item.text) })),
      });
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      throw new MemoryError('arbiter', 'the classifier failed', { cause: error });
    }
    if (!validateSchema(this.selectionSchema, output)) throw new MemoryError('arbiter', 'the classifier output is malformed');
    const chosen = nonempty.find(([member]) => member === output.member);
    if (!chosen) throw new MemoryError('invalid-selection', `the classifier selected ${output.member}`);
    return chosen;
  }
}

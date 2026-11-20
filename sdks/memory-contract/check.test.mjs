// Self-check of the Memory contract fixtures. Run with:
//   node --test sdks/memory-contract/check.test.mjs
// It validates the fixture structure, runs the semantic cases through the test-only reference
// memory, and runs the SQLite value and workload cases on node:sqlite. Passing it shows that the
// fixtures agree with the contract and with a real SQLite engine. It proves nothing about any SDK,
// Golem runtime artifact or tool.

import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import {
  Allowance,
  Cancelled,
  Composite,
  MemoryError,
  ReferenceMemory,
  assemble,
  isObservationId,
  validateSchema,
} from './reference-memory.mjs';

const load = (relative) => JSON.parse(readFileSync(new URL(relative, import.meta.url), 'utf8'));

const semantic = load('./fixtures/semantic-v1.json');
const traceVocabulary = load('./fixtures/trace-v1.json');
const defaults = load('./fixtures/defaults-v1.json');
const prompts = load('./fixtures/prompts-v1.json');
const values = load('./fixtures/values-v1.json');
const storage = load('./fixtures/storage-v1.json');
const wireForm = load('../../test-data/rest-wire-form/v1.json');

const defaultLimits = Object.fromEntries(Object.entries(defaults.limits).map(([name, limit]) => [name, limit.default]));
const flush = () => new Promise((resolve) => setImmediate(resolve));
const hashCounter = (text) => [...text].filter((character) => character === '#').length;

function assertUniqueIds(list, label) {
  const ids = list.map((entry) => entry.id);
  assert.equal(new Set(ids).size, ids.length, `${label} IDs are unique`);
  assert.ok(ids.every((id) => typeof id === 'string' && id !== ''), `${label} IDs are nonempty`);
}

// ---------------------------------------------------------------------------------------------
// Shared Golem schema encoding

const wireKinds = new Set(wireForm.roundtrip.map((entry) => entry.wire.kind));
const S64 = /^-?(0|[1-9][0-9]*)$/;

function assertWireLeaf(leaf, label) {
  assert.ok(wireKinds.has(leaf.kind), `${label}: ${leaf.kind} is a kind of the REST wire form`);
  switch (leaf.kind) {
    case 's64': {
      assert.match(leaf.value, S64, `${label}: canonical s64 text`);
      const value = BigInt(leaf.value);
      assert.ok(value >= -(2n ** 63n) && value < 2n ** 63n, `${label}: s64 range`);
      break;
    }
    case 'f32':
    case 'f64':
      assert.ok(
        typeof leaf.value === 'number' || ['nan', 'positive-infinity', 'negative-infinity'].includes(leaf.value?.$float),
        `${label}: float value`,
      );
      break;
    case 'string':
      assert.equal(typeof leaf.value, 'string', `${label}: string value`);
      break;
    case 'binary':
      assert.ok(leaf.value.bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255), `${label}: bytes`);
      break;
    case 'list':
      leaf.value.elements.forEach((element, index) => assertWireLeaf(element, `${label}[${index}]`));
      break;
    default:
      assert.fail(`${label}: the fixtures use no ${leaf.kind} leaf`);
  }
}

const floatOf = (value) =>
  typeof value === 'number' ? value : { nan: NaN, 'positive-infinity': Infinity, 'negative-infinity': -Infinity }[value.$float];

function floatLeaf(number) {
  if (Number.isNaN(number)) return { kind: 'f64', value: { $float: 'nan' } };
  if (number === Infinity) return { kind: 'f64', value: { $float: 'positive-infinity' } };
  if (number === -Infinity) return { kind: 'f64', value: { $float: 'negative-infinity' } };
  return { kind: 'f64', value: number };
}

function decodeCell(cell) {
  switch (cell.storageClass) {
    case 'null':
      return null;
    case 'integer':
      return BigInt(cell.value.value);
    case 'real':
      return floatOf(cell.value.value);
    case 'text':
      return cell.value.value;
    case 'blob':
      return Uint8Array.from(cell.value.value.bytes);
    default:
      assert.fail(`unknown storage class ${cell.storageClass}`);
  }
}

function encodeCell(value) {
  if (value === null) return { storageClass: 'null' };
  if (typeof value === 'bigint') return { storageClass: 'integer', value: { kind: 's64', value: value.toString() } };
  if (typeof value === 'number') return { storageClass: 'real', value: floatLeaf(value) };
  if (typeof value === 'string') return { storageClass: 'text', value: { kind: 'string', value } };
  if (value instanceof Uint8Array) return { storageClass: 'blob', value: { kind: 'binary', value: { bytes: [...value] } } };
  assert.fail(`unexpected SQLite value ${value}`);
}

function assertCell(cell, label) {
  assert.ok(['null', 'integer', 'real', 'text', 'blob'].includes(cell.storageClass), `${label}: storage class`);
  if (cell.storageClass === 'null') {
    assert.equal(cell.value, undefined, `${label}: null has no value`);
    return;
  }
  const expectedKind = { integer: 's64', real: 'f64', text: 'string', blob: 'binary' }[cell.storageClass];
  assert.equal(cell.value.kind, expectedKind, `${label}: leaf kind`);
  assertWireLeaf(cell.value, label);
}

const openSqlite = (path = ':memory:', options = {}) => new DatabaseSync(path, { readBigInts: true, ...options });

function rows(db, sql, params = []) {
  const statement = db.prepare(sql);
  statement.setReturnArrays(true);
  return statement.all(...params.map(decodeCell)).map((row) => row.map(encodeCell));
}

function applyProfile(db) {
  for (const pragma of storage.sqliteProfile.everyOpen) db.exec(pragma.sql);
}

// ---------------------------------------------------------------------------------------------
// Semantic cases

class ScriptedModel {
  constructor(role, script, env) {
    this.role = role;
    this.script = script;
    this.env = env;
  }

  async call({ source, signal }) {
    const { role, env } = this;
    const call = (env.counts[role] = (env.counts[role] ?? 0) + 1);
    const entry = this.script[call - 1];
    assert.ok(entry, `the ${role} script has no entry for call ${call}`);
    env.trace.push(source === undefined ? { event: 'model-start', role, call } : { event: 'model-start', role, call, source });
    const end = (outcome) => env.trace.push({ event: 'model-end', role, call, outcome });
    if (signal.aborted) {
      end('cancelled');
      throw new Cancelled();
    }
    let step = entry;
    if (entry.gate) {
      await new Promise((resolve, reject) => {
        const onAbort = () => {
          end('cancelled');
          reject(new Cancelled());
        };
        signal.addEventListener('abort', onAbort, { once: true });
        env.gate(entry.gate).promise.then(() => {
          signal.removeEventListener('abort', onAbort);
          resolve();
        });
      });
      step = entry.then;
    }
    if (step.error) {
      end('error');
      throw new Error(`scripted ${role} failure`);
    }
    if (step.malformed) {
      end('malformed');
      return {};
    }
    if (role === 'summarizer') {
      end(step.text === '' ? 'empty' : 'ok');
      return { summary: step.text };
    }
    end('ok');
    return role === 'classifier' ? { member: step.select } : { vector: step.vector ?? [] };
  }
}

function resolver(env) {
  const resolve = (value) => {
    if (typeof value === 'string' && value.startsWith('@')) {
      assert.ok(Object.hasOwn(semantic.ids, value.slice(1)), `alias ${value} is defined`);
      return semantic.ids[value.slice(1)];
    }
    if (typeof value === 'string' && value.startsWith('$')) {
      assert.ok(Object.hasOwn(env.captures, value.slice(1)), `${value} is captured before use`);
      return env.captures[value.slice(1)];
    }
    if (Array.isArray(value)) return value.map(resolve);
    if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, resolve(v)]));
    return value;
  };
  return resolve;
}

const callerOf = (alias) => {
  if (alias === undefined || alias === null) return null;
  assert.ok(Object.hasOwn(semantic.callers, alias), `caller ${alias} is defined`);
  return semantic.callers[alias];
};

const projectItem = (item) => (item.origins ? { text: item.text, sources: item.sources, origins: item.origins } : { text: item.text, sources: item.sources });

function project(op, value) {
  switch (op) {
    case 'recall':
      return value.map(projectItem);
    case 'context':
      return { items: value.items.map(projectItem), tokens: value.tokens };
    case 'tools':
      return { roles: value.tools.map((tool) => tool.role) };
    case 'assemble':
      return null;
    default:
      return value;
  }
}

function buildCase(c, env) {
  const models = Object.fromEntries(
    ['summarizer', 'embedder', 'classifier'].map((role) => [role, new ScriptedModel(role, c.models?.[role] ?? [], env)]),
  );
  for (const role of Object.keys(c.models ?? {})) assert.ok(models[role], `known model role ${role}`);
  const resolve = resolver(env);
  const members = new Map();
  for (const [name, config] of Object.entries(c.members)) {
    for (const key of Object.keys(config)) {
      assert.ok(['summarizer', 'queryEmbedder', 'items', 'faults', 'limits'].includes(key), `member option ${key}`);
    }
    members.set(
      name,
      new ReferenceMemory(name, resolve(config), {
        models,
        trace: env.trace,
        limits: { ...defaultLimits, ...config.limits },
        summarySchema: prompts.prompts.summarizer.outputSchema,
      }),
    );
  }
  const memories = new Map(members);
  const construction = new Map();
  for (const [name, config] of Object.entries(c.memories ?? {})) {
    const form = ['product', 'fallback', 'arbiter'].find((key) => config[key]);
    assert.ok(form, `${name} names a composition form`);
    const parts = config[form].map((member) => {
      assert.ok(memories.has(member), `${name} member ${member} is defined earlier`);
      const memory = memories.get(member);
      memory.traced = memory instanceof ReferenceMemory || memory.traced;
      return [member, memory];
    });
    try {
      memories.set(
        name,
        new Composite(form, parts, {
          limits: { ...defaultLimits, ...config.limits },
          classifier: models.classifier,
          candidateSchema: prompts.prompts.arbiter.outputSchema,
        }),
      );
      construction.set(name, { ok: true });
    } catch (error) {
      assert.ok(error instanceof MemoryError, `construction of ${name} fails with a MemoryError`);
      construction.set(name, { error });
    }
  }
  return { members, memories, construction };
}

function assertOutcome(outcome, expect, label, env, op) {
  const resolve = resolver(env);
  if (outcome.error && !(outcome.error instanceof MemoryError) && !(outcome.error instanceof Cancelled)) throw outcome.error;
  if (expect === undefined) {
    assert.ok(!outcome.error, `${label}: unexpected failure ${outcome.error?.kind ?? ''} ${outcome.error?.message ?? ''}`);
    return;
  }
  if (Object.hasOwn(expect, 'ok')) {
    assert.ok(!outcome.error, `${label}: expected success, got ${outcome.error?.kind} ${outcome.error?.message}`);
    const capture = expect.ok?.capture;
    if (capture !== undefined) {
      assert.ok(isObservationId(outcome.value), `${label}: an observation ID is a UUID`);
      env.captures[capture] = outcome.value;
      return;
    }
    assert.deepStrictEqual(project(op, outcome.value), resolve(expect.ok), label);
  } else if (expect.error) {
    assert.ok(outcome.error instanceof MemoryError, `${label}: expected a ${expect.error.kind} error`);
    const { kind, member, limit } = outcome.error;
    assert.deepStrictEqual({ kind, member, limit }, { member: undefined, limit: undefined, ...expect.error }, label);
  } else if (expect.cancelled) {
    assert.ok(outcome.error instanceof Cancelled, `${label}: expected cancellation`);
  } else {
    assert.fail(`${label}: unknown expectation`);
  }
}

function assertState(expected, members, env, label) {
  const resolve = resolver(env);
  for (const [name, state] of Object.entries(expected)) {
    const member = members.get(name);
    assert.ok(member, `${label}: state of unknown member ${name}`);
    for (const [key, value] of Object.entries(state)) {
      switch (key) {
        case 'sources':
          assert.deepStrictEqual(
            [...member.sources.values()].map(({ id, parts, caller }) => ({ id, parts, caller })),
            resolve(value).map((source) => ({ ...source, caller: callerOf(source.caller) })),
            `${label}: ${name} sources`,
          );
          break;
        case 'derived':
          assert.deepStrictEqual(member.derived.map(({ source, kind, text }) => ({ source, kind, text })), resolve(value), `${label}: ${name} derived`);
          break;
        case 'items':
          assert.deepStrictEqual(member.items.map(({ text, sources }) => ({ text, sources })), resolve(value), `${label}: ${name} items`);
          break;
        case 'derivedAmong': {
          const sources = new Set(resolve(value.sources));
          assert.equal(member.derived.filter((entry) => sources.has(entry.source)).length, value.count, `${label}: ${name} derived among`);
          break;
        }
        default:
          assert.fail(`${label}: unknown state key ${key}`);
      }
    }
  }
}

async function runCase(c) {
  const gates = new Map();
  const env = {
    trace: [],
    counts: {},
    captures: {},
    gate(name) {
      if (!gates.has(name)) {
        let release;
        const promise = new Promise((resolve) => (release = resolve));
        gates.set(name, { promise, release });
      }
      return gates.get(name);
    },
  };
  const resolve = resolver(env);
  const { members, memories, construction } = buildCase(c, env);

  if (c.expectConstruction) {
    for (const [name, expect] of Object.entries(c.expectConstruction)) {
      const outcome = construction.get(name);
      if (expect === 'ok') {
        assert.ok(outcome.ok, `${name} constructs, got ${outcome.error?.kind}`);
      } else {
        assert.ok(outcome.error, `${name} fails to construct`);
        const { kind, limit } = outcome.error;
        assert.deepStrictEqual({ kind, limit }, { limit: undefined, ...expect.error }, `${name} construction error`);
      }
    }
    return;
  }
  for (const [name, outcome] of construction) assert.ok(outcome.ok, `${name} constructs`);

  const controllers = new Map();
  const signal = (name) => {
    if (name === undefined) return new AbortController().signal;
    if (!controllers.has(name)) controllers.set(name, new AbortController());
    return controllers.get(name).signal;
  };
  const stored = new Map();
  const started = new Map();

  const perform = (step) => {
    const ctx = { caller: callerOf(step.caller), signal: signal(step.signal) };
    if (step.op === 'assemble') return assemble(step.toolsets.map((name) => stored.get(name)));
    if (step.op === 'tool') {
      const tool = stored.get(step.toolset).tools.find((entry) => entry.role === step.role);
      return tool.invoke(resolve(step.input), ctx);
    }
    const memory = memories.get(step.memory);
    assert.ok(memory, `memory ${step.memory} is defined`);
    const input = resolve(step.input);
    switch (step.op) {
      case 'observe':
      case 'recall':
        return memory[step.op](input, ctx);
      case 'context':
        return memory.context({ ...input, measureTokens: hashCounter }, ctx);
      case 'forget': {
        const references = step.input.references?.map(({ result, index }) => stored.get(result)[index].ref);
        return memory.forget(references ? { ...input, references } : input, ctx);
      }
      case 'tools':
        return memory.tools(input);
      default:
        assert.fail(`unknown op ${step.op}`);
    }
  };
  const settle = async (run) => {
    try {
      return { value: await run() };
    } catch (error) {
      return { error };
    }
  };

  for (const [index, step] of c.steps.entries()) {
    const label = `${c.id} step ${index + 1}`;
    if (step.start) {
      started.set(step.start, settle(() => perform(step)));
      await flush();
    } else if (step.await) {
      assert.ok(started.has(step.await), `${label}: ${step.await} was started`);
      const outcome = await started.get(step.await);
      started.delete(step.await);
      assertOutcome(outcome, step.expect, label, env, 'observe');
    } else if (step.cancel) {
      controllers.get(step.cancel).abort();
      await flush();
    } else if (step.release) {
      env.gate(step.release).release();
      await flush();
    } else if (step.op) {
      const outcome = await settle(() => perform(step));
      assertOutcome(outcome, step.expect, label, env, step.op);
      if (step.as) stored.set(step.as, outcome.value);
    } else if (step.expectState) {
      assertState(step.expectState, members, env, label);
    } else {
      assert.fail(`${label}: unknown step`);
    }
  }
  assert.deepStrictEqual([...started.keys()], [], `${c.id}: every started operation is awaited`);
  if (c.expectState) assertState(c.expectState, members, env, c.id);
  if (c.expectTrace) assert.deepStrictEqual(env.trace, resolve(c.expectTrace), `${c.id}: trace`);
  for (const [role, count] of Object.entries(c.expectModelCalls ?? {})) {
    assert.equal(env.counts[role] ?? 0, count, `${c.id}: ${role} calls`);
  }
}

test('semantic fixture structure', () => {
  assertUniqueIds(semantic.cases, 'semantic case');
  assert.ok(Object.values(semantic.ids).every(isObservationId), 'aliases name UUIDs');
  const vocabulary = traceVocabulary.events;
  for (const c of semantic.cases) {
    assert.ok(c.description, `${c.id} has a description`);
    for (const event of c.expectTrace ?? []) {
      assert.ok(Object.hasOwn(vocabulary, event.event), `${c.id}: ${event.event} is in the trace vocabulary`);
      const allowed = new Set(['event', ...vocabulary[event.event].fields]);
      assert.ok(Object.keys(event).every((key) => allowed.has(key)), `${c.id}: ${event.event} fields`);
    }
  }
});

for (const c of semantic.cases) {
  test(`semantic ${c.id}`, () => runCase(c));
}

// ---------------------------------------------------------------------------------------------
// Finite defaults

test('defaults name finite boundaries for every limit', () => {
  assertUniqueIds(defaults.boundaries, 'boundary');
  for (const [name, limit] of Object.entries(defaults.limits)) {
    assert.ok(Number.isInteger(limit.default) && limit.default > 0, `${name} is a positive integer`);
    const cases = defaults.boundaries.filter((entry) => entry.limit === name);
    assert.equal(cases.length, 2, `${name} has two boundary cases`);
    assert.equal(cases[0].value, limit.default, `${name} at the limit`);
    assert.equal(cases[1].value, limit.default + 1, `${name} beyond the limit`);
    if (name === 'graphHops') {
      assert.deepStrictEqual([cases[0].expect, cases[1].expect], ['returned', 'not-returned']);
    } else {
      assert.equal(cases[0].expect, 'ok');
      assert.deepStrictEqual(cases[1].expect, { error: { kind: 'limit', limit: name } });
    }
  }
});

const boundaryOutcome = async (run) => {
  try {
    await run();
    return 'ok';
  } catch (error) {
    assert.ok(error instanceof MemoryError, 'limits fail with a MemoryError');
    return { error: { kind: error.kind, limit: error.limit } };
  }
};

const referenceEnv = () => ({
  models: {},
  trace: [],
  limits: defaultLimits,
  summarySchema: prompts.prompts.summarizer.outputSchema,
});
const lexicalCtx = () => ({ caller: null, signal: new AbortController().signal });

test('candidate and fan-out boundaries on the reference memory', async () => {
  for (const boundary of defaults.boundaries) {
    let run;
    if (boundary.limit === 'candidateRows') {
      const items = Array.from({ length: boundary.value }, (_, index) => ({ text: `atlas ${index}`, sources: [semantic.ids.S1] }));
      const memory = new ReferenceMemory('m', { items }, referenceEnv());
      run = () => memory.recall({ query: 'atlas', limit: 1 }, lexicalCtx());
    } else if (boundary.limit === 'candidateBytes') {
      const items = [{ text: `atlas ${'x'.repeat(boundary.value - 6)}`, sources: [semantic.ids.S1] }];
      const memory = new ReferenceMemory('m', { items }, referenceEnv());
      run = () => memory.recall({ query: 'atlas', limit: 1 }, lexicalCtx());
    } else if (boundary.limit === 'memberFanOut') {
      const members = Array.from({ length: boundary.value }, (_, index) => [`m${index}`, new ReferenceMemory(`m${index}`, {}, referenceEnv())]);
      run = async () => new Composite('product', members, { limits: defaultLimits });
    } else {
      continue;
    }
    assert.deepStrictEqual(await boundaryOutcome(run), boundary.expect, boundary.id);
  }
  const allowance = new Allowance(defaultLimits);
  assert.throws(() => allowance.take('x'.repeat(defaultLimits.candidateBytes + 1)), /candidate bytes/);
});

// ---------------------------------------------------------------------------------------------
// Prompts and outputs

const escape = (text) => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
const renderParts = (parts) => parts.map((part, index) => `<part id="p${index + 1}">${escape(part)}</part>`).join('\n');
const renderCandidates = (candidates) =>
  candidates
    .filter((candidate) => candidate.items.length > 0)
    .map((candidate) => [`<candidate member="${candidate.member}">`, ...candidate.items.map((item) => `<item>${escape(item)}</item>`), '</candidate>'].join('\n'))
    .join('\n');

function classifyOutput(entry) {
  const prompt = prompts.prompts[entry.prompt];
  if (!validateSchema(prompt.outputSchema, entry.output)) return 'malformed';
  if (entry.prompt === 'extractor') {
    const ids = entry.output.entities.map((entity) => entity.id);
    const parts = new Set(entry.parts.map((_, index) => `p${index + 1}`));
    if (new Set(ids).size !== ids.length) return 'malformed';
    for (const assertion of entry.output.assertions) {
      if (!ids.includes(assertion.subject) || !ids.includes(assertion.object)) return 'malformed';
      if (!assertion.parts.every((part) => parts.has(part))) return 'malformed';
    }
  }
  if (entry.prompt === 'arbiter' && !entry.members.includes(entry.output.member)) return 'invalid-selection';
  return 'valid';
}

test('default prompts use exactly their declared inputs', () => {
  const identities = Object.values(prompts.prompts).map((prompt) => prompt.identity);
  assert.equal(new Set(identities).size, identities.length, 'prompt identities are unique');
  for (const [name, prompt] of Object.entries(prompts.prompts)) {
    const placeholders = [...prompt.text.matchAll(/\{\{(\w+)\}\}/g)].map((match) => match[1]);
    assert.deepStrictEqual([...new Set(placeholders)].sort(), [...prompt.inputs].sort(), `${name} placeholders`);
    assert.match(prompt.text, /data, not instructions/, `${name} treats content as data`);
  }
});

test('prompt rendering cases', () => {
  assertUniqueIds(prompts.renderingCases, 'rendering case');
  for (const entry of prompts.renderingCases) {
    if (entry.input.parts) assert.equal(renderParts(entry.input.parts), entry.parts, entry.id);
    if (entry.input.query) assert.equal(escape(entry.input.query), entry.query, entry.id);
    if (entry.input.candidates) assert.equal(renderCandidates(entry.input.candidates), entry.candidates, entry.id);
  }
});

test('prompt output cases', () => {
  assertUniqueIds(prompts.outputCases, 'output case');
  for (const entry of prompts.outputCases) assert.equal(classifyOutput(entry), entry.expect, entry.id);
});

// ---------------------------------------------------------------------------------------------
// Values on node:sqlite

test('value fixtures use the shared Golem schema encoding', () => {
  assertUniqueIds(values.sqlite.cells, 'SQLite cell');
  for (const cell of values.sqlite.cells) {
    assertCell(cell.bind, `${cell.id} bind`);
    assertCell(cell.read, `${cell.id} read`);
  }
  assertUniqueIds(values.cozo.values, 'Cozo value');
  for (const entry of values.cozo.values) {
    assert.equal(entry.schemaType, 'open', `${entry.id}: the producer defines the schema type`);
    assert.ok(entry.cozo && entry.expect.cozoKind, `${entry.id} names a Cozo kind`);
    if (entry.expect.leaf) assertWireLeaf(entry.expect.leaf, entry.id);
    for (const element of entry.expect.elements ?? []) assertWireLeaf(element.leaf ?? element, entry.id);
    if (entry.expect.timestampMicros) assertWireLeaf(entry.expect.timestampMicros, entry.id);
  }
});

test('SQLite cells round-trip on node:sqlite', () => {
  for (const cell of values.sqlite.cells) {
    const db = openSqlite();
    db.exec(cell.column === 'real' ? 'CREATE TABLE t(v REAL)' : 'CREATE TABLE t(v)');
    db.prepare('INSERT INTO t VALUES (?)').run(decodeCell(cell.bind));
    const [[read]] = rows(db, 'SELECT v FROM t');
    const [[storageClass]] = rows(db, 'SELECT typeof(v) FROM t');
    assert.deepStrictEqual(read, cell.read, cell.id);
    assert.equal(storageClass.value.value, cell.read.storageClass, `${cell.id} storage class`);
    db.close();
  }
});

test('SQLite batches are atomic on node:sqlite', () => {
  assertUniqueIds(values.sqlite.batches, 'batch');
  for (const batch of values.sqlite.batches) {
    const db = openSqlite();
    for (const sql of batch.setup) db.exec(sql);
    db.exec('BEGIN');
    let failed;
    for (const [index, statement] of batch.statements.entries()) {
      try {
        db.prepare(statement.sql).run(...statement.params.map(decodeCell));
      } catch {
        failed = index;
        break;
      }
    }
    db.exec(failed === undefined ? 'COMMIT' : 'ROLLBACK');
    assert.deepStrictEqual(failed === undefined ? { ok: true } : { failedStatement: failed }, batch.expect, batch.id);
    assert.deepStrictEqual(rows(db, batch.check), batch.rows, `${batch.id} rows`);
    db.close();
  }
});

// ---------------------------------------------------------------------------------------------
// Storage profile and workloads on node:sqlite

function withDirectory(body) {
  const directory = mkdtempSync(join(tmpdir(), 'memory-contract-'));
  try {
    return body(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const scalar = (db, sql) => {
  const [[cell]] = rows(db, sql);
  return cell.storageClass === 'integer' ? Number(cell.value.value) : cell.value.value;
};

test('SQLite connection profile on node:sqlite', () => {
  const profile = storage.sqliteProfile;
  const version = scalar(openSqlite(), 'SELECT sqlite_version()').split('.').map(Number);
  const minimum = profile.ftsReaderMinimumVersion.split('.').map(Number);
  assert.ok(version[0] > minimum[0] || (version[0] === minimum[0] && version[1] >= minimum[1]), 'node:sqlite reads secure-delete FTS5');
  withDirectory((directory) => {
    const path = join(directory, 'memory.db');
    const db = openSqlite(path);
    applyProfile(db);
    for (const pragma of profile.everyOpen) assert.equal(scalar(db, pragma.verify), pragma.expect, pragma.sql);
    const [journal, fts] = profile.durableInitialization;
    assert.equal(scalar(db, journal.sql), journal.expect);
    db.exec('CREATE VIRTUAL TABLE chunks USING fts5(body)');
    db.exec(fts.fts5.replaceAll('<fts>', 'chunks'));
    assert.equal(scalar(db, fts.verify.replaceAll('<fts>', 'chunks')), fts.expect);
    db.close();
    const reader = openSqlite(path, { readOnly: true });
    applyProfile(reader);
    for (const pragma of profile.everyOpen) assert.equal(scalar(reader, pragma.verify), pragma.expect, `read-only ${pragma.sql}`);
    assert.equal(scalar(reader, profile.readOnlyOpen[0].verify), profile.readOnlyOpen[0].expect);
    reader.close();
  });
});

test('SQLite workloads on node:sqlite', () => {
  assertUniqueIds(storage.sqliteWorkloads, 'SQLite workload');
  for (const workload of storage.sqliteWorkloads) {
    const db = openSqlite();
    applyProfile(db);
    for (const statement of workload.setup) db.exec(statement.sql);
    for (const [index, query] of workload.queries.entries()) {
      assert.deepStrictEqual(rows(db, query.sql, query.params), query.rows, `${workload.id} query ${index + 1}`);
    }
    db.close();
  }
});

test('cosine scans over bounded pages on node:sqlite', () => {
  for (const scan of storage.cosineScans) {
    const db = openSqlite();
    for (const statement of scan.setup) db.exec(statement.sql);
    const insert = db.prepare('INSERT INTO vec VALUES (?, ?)');
    for (const { id, f32 } of scan.vectors) {
      const bytes = new Uint8Array(f32.length * 4);
      const view = new DataView(bytes.buffer);
      f32.forEach((value, index) => view.setFloat32(index * 4, value, true));
      insert.run(BigInt(id), bytes);
    }
    const page = db.prepare(scan.pageQuery);
    const norm = (vector) => Math.hypot(...vector);
    const scored = [];
    let after = 0n;
    let pages = 0;
    for (;;) {
      const found = page.all(after, BigInt(scan.pageSize));
      pages += 1;
      for (const row of found) {
        const view = new DataView(row.v.buffer, row.v.byteOffset, row.v.byteLength);
        const vector = Array.from({ length: row.v.byteLength / 4 }, (_, index) => view.getFloat32(index * 4, true));
        const dot = vector.reduce((sum, value, index) => sum + value * scan.query[index], 0);
        scored.push({ id: Number(row.id), similarity: dot / (norm(vector) * norm(scan.query)) });
        after = row.id;
      }
      if (found.length < scan.pageSize) break;
    }
    scored.sort((a, b) => b.similarity - a.similarity || a.id - b.id);
    assert.deepStrictEqual({ ids: scored.slice(0, scan.top).map((entry) => entry.id), pagesRead: pages }, scan.expect, scan.id);
    db.close();
  }
});

test('every SQLite refusal form mutates a writable database', () => {
  assertUniqueIds(storage.sqliteMutatingForms, 'SQLite refusal form');
  for (const form of storage.sqliteMutatingForms) {
    withDirectory((directory) => {
      const path = join(directory, 'memory.db');
      const setup = openSqlite(path);
      setup.exec('PRAGMA journal_mode = DELETE');
      for (const statement of storage.sqliteRefusalSetup) setup.exec(statement.sql);
      setup.close();
      const state = () => ({ counter: readFileSync(path).readUInt32BE(24), files: readdirSync(directory).sort() });
      const before = state();
      const db = openSqlite(path);
      db.exec(form.sql.replaceAll('{dir}', directory));
      db.close();
      assert.notDeepStrictEqual(state(), before, `${form.id} writes the database or another file`);
    });
  }
});

// ---------------------------------------------------------------------------------------------
// Cozo, lifecycle, hook and card descriptors

test('Cozo workload structure', () => {
  const cozo = storage.cozoWorkloads;
  assert.ok(cozo.setup.every((script) => typeof script === 'string' && script !== ''));
  assertUniqueIds(cozo.queries, 'Cozo query');
  for (const query of cozo.queries) {
    assert.ok(query.script, query.id);
    for (const [name, leaf] of Object.entries(query.params)) {
      assert.match(query.script, new RegExp(`\\$${name}\\b`), `${query.id} uses $${name}`);
      assertWireLeaf(leaf, `${query.id} $${name}`);
    }
  }
  assertUniqueIds([...cozo.mutatingForms, cozo.freshness], 'Cozo form');
  assert.ok(cozo.mutatingForms.every((form) => form.script.includes(':')), 'Cozo refusal forms are commands');
});

test('lifecycle, hook and card scenarios', () => {
  const scenarios = [...storage.lifecycle, ...storage.hooks.scenarios, ...storage.cards.scenarios];
  assertUniqueIds(scenarios, 'scenario');
  for (const scenario of scenarios) {
    assert.ok(scenario.scenario && scenario.pass, `${scenario.id} has a scenario and a pass condition`);
  }
  for (const scenario of storage.lifecycle) {
    assert.ok(scenario.engines.every((engine) => ['sqlite', 'cozo'].includes(engine)), scenario.id);
  }
});

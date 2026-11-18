// Execute the real generated core exports with already-resolved corpus values.
// This is an ABI/SDK check, not a replacement for Golem's resolver or oplog tests.
import assert from 'node:assert/strict';
import fs from 'node:fs';

const corpus = JSON.parse(fs.readFileSync(new URL('../../test-data/config-conformance/v1.json', import.meta.url)));
const wasm = fs.readFileSync(process.argv[2]);
const module = new WebAssembly.Module(wasm);

function expectedType({kind, value: v}) {
  switch (kind) {
    case 'ref': return ['ref', v.id];
    case 'record': return ['record', v.fields.map(f => [f.name, expectedType(f.body)])];
    case 'tuple': return ['tuple', v.elements.map(expectedType)];
    case 'option': return ['option', expectedType(v.inner)];
    case 'union': return ['union', v.spec.branches.map(b => [b.tag, expectedType(b.body), b.discriminator.value.prefix])];
    default: return kind;
  }
}

function guest(values, {deny = false, malformed = false} = {}) {
  let instance, context = 0, nextSet = 1, returned;
  const captured = structuredClone(values);
  const view = () => new DataView(instance.exports.memory.buffer);
  const u32 = p => view().getUint32(p, true);
  const byte = p => view().getUint8(p);
  const put = (p, n) => view().setUint32(p, n, true);
  const alloc = n => {
    const p = instance.exports.cabi_realloc(0, 0, 8, Math.max(n, 8));
    new Uint8Array(instance.exports.memory.buffer).fill(0, p, p + Math.max(n, 8));
    return p;
  };
  const str = p => Buffer.from(instance.exports.memory.buffer, u32(p), u32(p + 4) * 2).toString('utf16le');
  const writeString = (p, s) => {
    const data = Buffer.from(s, 'utf16le'), ptr = alloc(data.length);
    new Uint8Array(instance.exports.memory.buffer).set(data, ptr);
    put(p, ptr); put(p + 4, s.length);
  };
  const list = (p, stride, read) => Array.from({length: u32(p + 4)}, (_, i) => read(u32(p) + stride * i));
  function graph(p) {
    const nodes = u32(p), nodeCount = u32(p + 4);
    const definitions = list(p + 8, 24, d => ({name: byte(d + 8) ? str(d + 12) : str(d), root: u32(d + 20)}));
    function type(index) {
      assert.ok(index < nodeCount, 'schema index belongs to owner graph');
      const n = nodes + index * 144;
      switch (byte(n)) {
        case 0: return ['ref', definitions[u32(n + 8)].name];
        case 1: return 'bool';
        case 4: return 's32';
        case 8: return 'u32';
        case 13: return 'string';
        case 14: return ['record', list(n + 8, 68, f => [str(f), type(u32(f + 8))])];
        case 18: return ['tuple', list(n + 8, 4, i => type(u32(i)))];
        case 19: return ['list', type(u32(n + 8))];
        case 22: return ['option', type(u32(n + 8))];
        case 32: return ['union', list(n + 8, 92, b => {
          assert.equal(byte(b + 12), 0, 'prefix discriminator');
          return [str(b), type(u32(b + 8)), str(b + 16)];
        })];
        default: throw Error(`unsupported fixture schema node ${byte(n)}`);
      }
    }
    return {type, root: type(u32(p + 16)), defs: Object.fromEntries(definitions.map(d => [d.name, type(d.root)]))};
  }
  function metadata(p) {
    const schema = graph(p + 28);
    const declarations = list(p + 224, 28, d => ({
      source: byte(d), path: list(d + 4, 8, str), type: schema.type(u32(d + 12)),
      defaultValue: byte(d + 16) ? JSON.parse(str(d + 20)) : undefined,
    }));
    return {name: str(p), defs: schema.defs, declarations};
  }
  function encode(type, value, defs) {
    const nodes = [];
    const push = (kind, fill = () => {}) => { nodes.push({kind, fill}); return nodes.length - 1; };
    const children = (kind, indices) => push(kind, p => {
      const data = alloc(indices.length * 4);
      indices.forEach((n, i) => put(data + i * 4, n));
      put(p + 8, data); put(p + 12, indices.length);
    });
    function add(t, v) {
      if (typeof t === 'string') {
        switch (t) {
          case 'string': return push(12, p => writeString(p + 8, v));
          case 'u32': return push(7, p => put(p + 8, v));
          case 's32': return push(3, p => put(p + 8, v));
          case 'bool': return push(0, p => put(p + 8, Number(v)));
          default: throw Error(`unsupported fixture value ${t}`);
        }
      }
      switch (t[0]) {
        case 'ref': return add(defs[t[1]], v);
        case 'record': return children(13, t[1].map(([name, body]) => add(body, v[name])));
        case 'tuple': return children(17, t[1].map((body, i) => add(body, v[i])));
        case 'list': return children(18, v.map(item => add(t[1], item)));
        case 'option': {
          const inner = v === null ? undefined : add(t[1], v);
          return push(21, p => { if (inner !== undefined) { put(p + 8, 1); put(p + 12, inner); } });
        }
        case 'union': {
          const [tag, body] = t[1].find(([, , prefix]) => v.startsWith(prefix));
          const child = add(body, v);
          return push(31, p => { writeString(p + 8, tag); put(p + 16, child); });
        }
        default: throw Error(`unsupported fixture compound ${t[0]}`);
      }
    }
    const root = add(type, value), ptr = alloc(nodes.length * 32);
    nodes.forEach(({kind, fill}, i) => { const p = ptr + i * 32; put(p, kind); fill(p); });
    return {ptr, length: nodes.length, root};
  }
  const imports = {};
  const implementations = {
    '$root': {
      '[context-get-0]': () => context,
      '[context-set-0]': n => { context = n; },
      '[waitable-set-new]': () => nextSet++,
      '[waitable-set-drop]': () => {},
    },
    '[export]golem:agent/guest@2.0.0': {
      '[task-return]initialize': (...args) => { returned = args; },
      '[task-return]invoke': (...args) => {
        returned = args;
        if (args[0] === 0 && args[1] === 1) {
          const node = args[2] + args[4] * 32;
          assert.equal(byte(node), 12, 'read result is a string');
          returned = JSON.parse(str(node + 8));
        }
      },
    },
    'golem:agent/invocation-context@2.0.0': {
      caller: result => {
        put(result, 3);
        put(result + 112, 3);
      },
    },
    'golem:agent/host@2.0.0': {
      'get-config-value': (keys, length, nodes, nodeCount, defs, defCount, root, result) => {
        const key = Array.from({length}, (_, i) => str(keys + i * 8));
        if (deny) { put(result, 1); put(result + 4, 0); return; }
        const g = alloc(20);
        [nodes, nodeCount, defs, defCount, root].forEach((n, i) => put(g + i * 4, n));
        const schema = graph(g);
        const value = key.reduce((v, name) => v[name], captured);
        assert.notEqual(value, undefined, 'host must supply every resolved field, including absence');
        const tree = malformed ? encode('string', 'wrong shape', {}) : encode(schema.root, value, schema.defs);
        put(result, 0); put(result + 4, tree.ptr); put(result + 8, tree.length); put(result + 12, tree.root);
      },
    },
  };
  for (const {module: mod, name} of WebAssembly.Module.imports(module)) {
    imports[mod] ??= {};
    imports[mod][name] = implementations[mod]?.[name] ?? (() => { throw Error(`unexpected host call ${mod}#${name}`); });
  }
  instance = new WebAssembly.Instance(module, imports);
  const discover = instance.exports['golem:agent/guest@2.0.0#discover-agent-types']();
  assert.equal(byte(discover), 0);
  const agents = list(discover + 4, 232, metadata);
  instance.exports['cabi_post_golem:agent/guest@2.0.0#discover-agent-types'](discover);
  function call(operation, name, input) {
    const tree = encode(['record', input.map((_, i) => [String(i), 'string'])], Object.fromEntries(input.map((v, i) => [i, v])), {});
    const args = alloc(8);
    writeString(args, name);
    returned = undefined;
    let status = instance.exports[`[async-lift]golem:agent/guest@2.0.0#${operation}`](u32(args), u32(args + 4), tree.ptr, tree.length, tree.root);
    for (let i = 0; status === 1 && i < 10; i++) {
      status = instance.exports[`[callback][async-lift]golem:agent/guest@2.0.0#${operation}`](0, 0, 0);
    }
    assert.equal(status, 0, 'export completed without suspended host operations');
    assert.notEqual(returned, undefined, 'export returned through task-return');
    return returned;
  }
  return {agents, initialize: (name, input = []) => call('initialize', name, input), read: () => call('invoke', 'read', [])};
}

const discovered = guest({}).agents;
const reference = discovered.find(a => a.name === corpus.reference.agentType);
const expected = corpus.schemas.reference;
assert.deepEqual(reference.defs, Object.fromEntries(expected.graph.defs.map(d => [d.name, expectedType(d.body)])));
assert.deepEqual(reference.declarations, expected.declarations.map(d => ({
  source: 0, path: d.path, type: expectedType(d.value_type),
  defaultValue: Object.hasOwn(d, 'default_value') ? JSON.parse(d.default_value) : undefined,
})));
const shapes = discovered.find(a => a.name === 'StaticShapeAgent');
assert.deepEqual(shapes.defs.Node, expectedType(corpus.schemas.recursive.graph.defs[0].body));
assert.deepEqual(shapes.declarations.find(d => d.path[0] === 'tree').defaultValue,
  JSON.parse(corpus.schemas.recursive.declarations[0].default_value));
assert.equal(shapes.declarations.find(d => d.path[0] === 'cleared').defaultValue, null);
assert.equal(shapes.declarations.find(d => d.path[0] === 'maxChars').defaultValue, 4000);
assert.equal(shapes.declarations.find(d => d.path[0] === 'enabled').defaultValue, false);
assert.deepEqual(shapes.declarations.find(d => d.path[0] === 'values').defaultValue, []);
assert.equal(reference.declarations.find(d => d.path[0] === 'omitted').defaultValue, undefined);
console.log('PASS: real guest metadata matches independent reference schemas/defaults, recursive Node and canonical names');

const readExpected = values => ({...values, choice: {[values.choice.startsWith('selected:') ? 'Text' : 'Other']: values.choice}});
for (const c of corpus.staticCases.filter(c => c.schema === 'reference' && c.expected)) {
  const resolved = structuredClone(c.expected);
  const g = guest(resolved);
  assert.equal(g.initialize(corpus.reference.agentType, [c.id])[0], 0);
  assert.deepEqual(g.read(), readExpected(c.expected), c.id);
  resolved.settings = {label: 'changed-after-initialization', count: 999};
  assert.deepEqual(g.read(), readExpected(c.expected), 'host capture stays fixed until an explicit update');
  const reconstructed = guest(c.expected);
  assert.equal(reconstructed.initialize(corpus.reference.agentType, [c.id])[0], 0);
  assert.deepEqual(reconstructed.read(), readExpected(c.expected), 'fresh guest captures same resolved values');
  console.log(`PASS: ${c.id} capture, pinned read and fresh-instance recapture`);
}
for (const key of ['expectedDeploymentRead', 'expectedRead']) {
  const v = corpus.reference[key];
  const g = guest({...v, choice: v.choice.Text});
  assert.equal(g.initialize(corpus.reference.agentType, [key])[0], 0);
  assert.deepEqual(g.read(), v);
  console.log(`PASS: corpus reference ${key}`);
}
const recursive = corpus.staticCases.find(c => c.id === 'static/recursive-default').expected.tree;
const g = guest({maxChars: 0, tree: recursive, cleared: null, enabled: false, values: []});
assert.equal(g.initialize('StaticShapeAgent')[0], 0);
assert.deepEqual(g.read(), {maxChars: 0, root: recursive.displayName, child: recursive.next.displayName, cleared: null, enabled: false, values: []});
console.log('PASS: recursive data, renamed field, zero, false, empty list and explicit null');
for (const options of [{deny: true}, {malformed: true}]) {
  const bad = guest(corpus.staticCases[0].expected, options);
  const result = bad.initialize(corpus.reference.agentType, ['invalid']);
  assert.equal(result[0], 1, 'failed host read/decoding is not repaired with a code default');
  console.log(`PASS: ${Object.keys(options)[0]} host value fails instead of falling back`);
}
console.log('Core-export SDK checks passed. Full Golem resolver/oplog reconstruction is a separate host acceptance test.');

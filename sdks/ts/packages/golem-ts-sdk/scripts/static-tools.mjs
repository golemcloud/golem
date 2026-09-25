import ts from 'typescript';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const sdkName = '@golemcloud/golem-ts-sdk';

function literal(value) {
  if (value === undefined) return 'undefined';
  if (typeof value === 'bigint') return `${value}n`;
  if (typeof value === 'number' && !Number.isFinite(value)) return String(value);
  if (Array.isArray(value)) return `[${value.map(literal).join(',')}]`;
  if (value !== null && typeof value === 'object')
    return `{${Object.entries(value)
      .map(([k, v]) => `${JSON.stringify(k)}:${literal(v)}`)
      .join(',')}}`;
  return JSON.stringify(value);
}

// Load only the SDK's metadata compiler in Node. Guest imports are unavailable:
// metadata evaluation must not perform host operations or run application code.
function metadataModules(runtime) {
  const cache = new Map();
  const load = (file) => {
    if (cache.has(file)) return cache.get(file).exports;
    const module = { exports: {} };
    cache.set(file, module);
    const require = (id) => {
      if (/^(golem:|wasi:|wasm-rquickjs:)/.test(id))
        return new Proxy(
          {},
          {
            get: (_target, key) => {
              if (key === '__esModule') return true;
              return () => {
                throw new Error(`Host operation ${id}.${String(key)} in static metadata`);
              };
            },
          },
        );
      if (id.startsWith('.')) return load(path.resolve(path.dirname(file), id));
      return createRequire(file)(id);
    };
    const code = ts.transpileModule(fs.readFileSync(file, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
      fileName: `${file}.ts`,
    }).outputText;
    new Function('require', 'module', 'exports', code)(require, module, module.exports);
    return module.exports;
  };
  for (const vendor of ['zod', 'valibot', 'arktype', 'effect'])
    load(path.join(runtime, `schema/${vendor}.mjs`));
  return {
    tool: load(path.join(runtime, 'tool.mjs')),
    http: load(path.join(runtime, 'http.mjs')),
    agent: {
      ...load(path.join(runtime, 'method.mjs')),
      defineAgent: (spec) => ({ name: spec.name, spec }),
    },
    runtime: load(path.join(runtime, 'runtime.mjs')),
    schema: {
      ...load(path.join(runtime, 'schema/public.mjs')),
      ...load(path.join(runtime, 'schema/markers.mjs')),
    },
    compile: load(path.join(runtime, 'schema/adapter.mjs')).compileSchema,
    encode: load(path.join(runtime, 'internal/tool/encoding.mjs')).encodeTool,
    graph: load(path.join(runtime, 'internal/schema-model/wit.mjs')).schemaGraphToWit,
  };
}

export function staticTools(config, runtime) {
  const program = ts.createProgram(config.fileNames, config.options);
  const checker = program.getTypeChecker();
  let usesDynamicModels = false;
  for (const source of program.getSourceFiles()) {
    if (source.isDeclarationFile || source.fileName.includes('/node_modules/')) continue;
    const inspect = (node) => {
      if (
        ts.isImportDeclaration(node) &&
        [sdkName + '/schema', sdkName + '/reflection'].includes(node.moduleSpecifier.text)
      )
        usesDynamicModels = true;
      if (
        ts.isPropertyAccessExpression(node) &&
        ['parsed', 'parts', 'dynamicClient'].includes(node.name.text)
      )
        usesDynamicModels = true;
      if (ts.isIdentifier(node)) {
        let symbol = checker.getSymbolAtLocation(node);
        if (symbol?.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
        if (
          [
            'defineAgentClient',
            'compileSchema',
            'SchemaRef',
            'parsed',
            'parts',
            'dynamicClient',
            'create',
          ].includes(symbol?.name) &&
          symbol.declarations?.some((d) => d.getSourceFile().fileName.endsWith('/dist/index.d.mts'))
        )
          usesDynamicModels = true;
      }
      if (
        ts.isElementAccessExpression(node) &&
        checker.getTypeAtLocation(node.expression).getProperty('dynamicClient')
      )
        usesDynamicModels = true;
      if (
        ts.isCallExpression(node) &&
        node.arguments.some((arg) => checker.getTypeAtLocation(arg).getProperty('dynamicClient'))
      )
        usesDynamicModels = true;
      ts.forEachChild(node, inspect);
    };
    inspect(source);
  }
  let modules;
  const metadata = () => (modules ??= metadataModules(runtime));
  const values = new Map();
  const evaluating = new Set();
  const schemaExpressions = new Map();
  const fail = (node, message) => {
    const source = node.getSourceFile();
    const pos = source.getLineAndCharacterOfPosition(node.getStart(source));
    throw new Error(`${source.fileName}:${pos.line + 1}: ${message}`);
  };
  const evaluate = (node, locals = new Map()) => {
    const value = evaluateExpression(node, locals);
    if (value && typeof value === 'object' && '~standard' in value)
      schemaExpressions.set(value, node.getText(node.getSourceFile()));
    return value;
  };
  const evaluateExpression = (node, locals) => {
    if (
      ts.isParenthesizedExpression(node) ||
      ts.isAsExpression(node) ||
      ts.isNonNullExpression(node) ||
      ts.isSatisfiesExpression(node)
    )
      return evaluate(node.expression, locals);
    if (ts.isStringLiteralLike(node) || ts.isNumericLiteral(node))
      return ts.isNumericLiteral(node) ? Number(node.text) : node.text;
    if (ts.isBigIntLiteral(node)) return BigInt(node.text.slice(0, -1));
    if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
    if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
    if (node.kind === ts.SyntaxKind.NullKeyword) return null;
    if (ts.isIdentifier(node)) {
      if (locals.has(node.text)) return locals.get(node.text);
      if (node.text === 'undefined') return undefined;
      let symbol = ts.isShorthandPropertyAssignment(node.parent)
        ? checker.getShorthandAssignmentValueSymbol(node.parent)
        : checker.getSymbolAtLocation(node);
      const declaration = symbol?.declarations?.[0];
      if (
        declaration &&
        (ts.isImportSpecifier(declaration) ||
          ts.isNamespaceImport(declaration) ||
          ts.isImportClause(declaration))
      ) {
        let parent = declaration;
        while (parent && !ts.isImportDeclaration(parent)) parent = parent.parent;
        const id = parent.moduleSpecifier.text;
        const name = ts.isImportSpecifier(declaration)
          ? (declaration.propertyName ?? declaration.name).text
          : 'default';
        if (id === sdkName) {
          if (ts.isNamespaceImport(declaration))
            return {
              ...metadata().tool,
              ...metadata().agent,
              ...metadata().schema,
              http: metadata().http,
            };
          if (name === 'http') return metadata().http;
          if (name in metadata().tool) return metadata().tool[name];
          if (name in metadata().agent) return metadata().agent[name];
          if (name in metadata().schema) return metadata().schema[name];
          fail(node, `SDK export ${name} is not a static tool metadata constructor`);
        }
        if (id === `${sdkName}/schema`) return metadata().schema[name];
        if (id.startsWith('.')) {
          const target = checker.getAliasedSymbol(symbol)?.valueDeclaration;
          if (target && ts.isVariableDeclaration(target) && target.initializer)
            return evaluate(target.initializer, locals);
          fail(node, `Imported metadata ${name} must be a const expression`);
        }
        if (!['zod', 'zod/v4', 'valibot', 'arktype', 'effect'].includes(id))
          fail(node, `Static metadata cannot execute imported application module ${id}`);
        const imported = createRequire(node.getSourceFile().fileName)(id);
        return ts.isNamespaceImport(declaration) ? imported : imported[name];
      }
      if (symbol?.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
      if (values.has(symbol)) return values.get(symbol);
      const decl = symbol?.valueDeclaration;
      if (
        !decl ||
        !ts.isVariableDeclaration(decl) ||
        !decl.initializer ||
        !(decl.parent.flags & ts.NodeFlags.Const)
      )
        fail(node, 'Tool metadata must be a statically evaluable const expression');
      if (evaluating.has(symbol)) fail(node, 'Cyclic eager metadata declaration');
      evaluating.add(symbol);
      try {
        const value = evaluate(decl.initializer, locals);
        values.set(symbol, value);
        return value;
      } finally {
        evaluating.delete(symbol);
      }
    }
    if (ts.isObjectLiteralExpression(node)) {
      const result = {};
      for (const field of node.properties) {
        if (ts.isSpreadAssignment(field)) Object.assign(result, evaluate(field.expression, locals));
        else if (ts.isShorthandPropertyAssignment(field))
          result[field.name.text] = evaluate(field.name, locals);
        else if (ts.isPropertyAssignment(field) && !ts.isComputedPropertyName(field.name))
          result[field.name.text] = evaluate(field.initializer, locals);
        else fail(field, 'Static metadata requires data properties');
      }
      return result;
    }
    if (ts.isArrayLiteralExpression(node))
      return node.elements.flatMap((item) =>
        ts.isSpreadElement(item) ? evaluate(item.expression, locals) : [evaluate(item, locals)],
      );
    if (ts.isPropertyAccessExpression(node))
      return evaluate(node.expression, locals)[node.name.text];
    if (ts.isArrowFunction(node) && !ts.isBlock(node.body))
      return (...args) => {
        const scope = new Map(locals);
        node.parameters.forEach((p, i) => {
          if (!ts.isIdentifier(p.name))
            fail(p, 'Static metadata callbacks require named parameters');
          scope.set(p.name.text, args[i]);
        });
        return evaluate(node.body, scope);
      };
    if (ts.isCallExpression(node)) {
      const receiver = ts.isPropertyAccessExpression(node.expression)
        ? evaluate(node.expression.expression, locals)
        : undefined;
      const fn =
        receiver === undefined
          ? evaluate(node.expression, locals)
          : receiver[node.expression.name.text];
      if (typeof fn !== 'function') fail(node, 'Static metadata call target is not a constructor');
      return Reflect.apply(
        fn,
        receiver,
        node.arguments.map((arg) => evaluate(arg, locals)),
      );
    }
    if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.MinusToken)
      return -evaluate(node.operand, locals);
    fail(node, 'Unsupported static metadata expression');
  };

  function codecSource(codec, declarations, seen = new Map(), defs = codec.graph.defs) {
    const key = codec.graph.root.body.tag === 'ref' ? codec.graph.root.body.id : codec.graph.root;
    if (seen.has(key)) return seen.get(key);
    const name = `c${declarations.length}`;
    seen.set(key, name);
    const slot = declarations.push('') - 1;
    let body = codec.graph.root.body;
    if (body.tag === 'ref') body = defs.get(body.id).body.body;
    const child = (type) => ({ graph: { defs, root: type } });
    const emit = (value) => codecSource(value, declarations, seen, defs);
    let read;
    let write;
    let tag = `${body.tag}-value`;
    if (codec.isUnit) {
      tag = 'tuple-value';
      read = 'if(n.val.length)throw new TypeError("expected unit");return undefined;';
      write =
        'if(v!==undefined)throw new TypeError("expected unit");return w.add({tag:"tuple-value",val:[]});';
    } else if (body.tag === 'record' || body.tag === 'tuple') {
      const items = codec.sourceSchema?._def?.items;
      const fields =
        body.tag === 'record'
          ? (codec.fields ?? body.fields.map((f) => ({ name: f.name, codec: child(f.body) })))
          : body.elements.map((t, i) => ({
              name: i,
              codec: items?.[i] ? metadata().compile(items[i]) : child(t),
            }));
      const children = fields.map((f) => emit(f.codec));
      const checks =
        body.tag === 'record'
          ? 'typeof v!=="object"||v===null||Array.isArray(v)'
          : `!Array.isArray(v)||v.length!==${fields.length}`;
      write = `if(${checks})throw new TypeError("expected ${body.tag}");return w.add({tag:${literal(tag)},val:[${fields.map((f, i) => `${children[i]}.write(v[${literal(f.name)}],w)`).join(',')}]});`;
      const fieldsRead = fields
        .map(
          (f, i) =>
            `${body.tag === 'record' ? literal(f.name) + ':' : ''}${children[i]}.read(r,n.val[${i}])`,
        )
        .join(',');
      read = `if(!Array.isArray(n.val)||n.val.length!==${fields.length})throw new TypeError("wrong field count");return ${body.tag === 'record' ? '{' : '['}${fieldsRead}${body.tag === 'record' ? '}' : ']'};`;
    } else if (body.tag === 'list' || body.tag === 'fixed-list') {
      const multimodal = codec.concrete?.tag === 'multimodal' ? codec.concrete : undefined;
      const c = multimodal ? undefined : emit(codec.listItem ?? child(body.element));
      const length = body.tag === 'fixed-list' ? `||v.length!==${body.length}` : '';
      const ctor = codec.concrete?.tag === 'typed-array' ? codec.concrete.constructor : undefined;
      if (multimodal) {
        const cases = multimodal.cases.map((entry) => emit(entry.codec));
        const names = multimodal.cases.map((entry) => entry.name);
        write = `if(!Array.isArray(v))throw new TypeError("expected multimodal list");return w.add({tag:"list-value",val:v.map(x=>{const i=${literal(names)}.indexOf(x?.tag);if(i<0)throw new TypeError("unknown multimodal case");return w.add({tag:"variant-value",val:{case_:i,payload:[${cases.join(',')}][i].write(x.value,w)}})});`;
        read = `if(!Array.isArray(n.val))throw new TypeError("invalid multimodal list");return n.val.map(i=>r.node(i,"variant-value",x=>{const c=x.val.case_;if(!Number.isInteger(c)||c<0||c>=${cases.length}||x.val.payload===undefined)throw new TypeError("invalid multimodal case");return {tag:${literal(names)}[c],value:[${cases.join(',')}][c].read(r,x.val.payload)}}));`;
      } else {
        write = `if(${ctor ? `!(v instanceof ${ctor})` : '!Array.isArray(v)'}${length})throw new TypeError("expected list");return w.add({tag:${literal(tag)},val:Array.from(v,x=>${c}.write(x,w))});`;
        read = `if(!Array.isArray(n.val)${body.tag === 'fixed-list' ? `||n.val.length!==${body.length}` : ''})throw new TypeError("invalid list");const v=n.val.map(i=>${c}.read(r,i));return ${ctor ? `new ${ctor}(v)` : 'v'};`;
      }
    } else if (body.tag === 'option') {
      const c = emit(codec.optionInner ?? child(body.element));
      const absent = codec.optionKind === 'nullable' ? 'null' : 'undefined';
      write = `return w.add({tag:"option-value",val:${codec.optionKind === 'nullish' ? 'v==null' : `v===${absent}`}?undefined:${c}.write(v,w)});`;
      read = `return n.val===undefined?${absent}:${c}.read(r,n.val);`;
    } else if (body.tag === 'result') {
      const ok = codec.resultOk ?? (body.ok && child(body.ok));
      const err = codec.resultErr ?? (body.err && child(body.err));
      const arm = (c) => (c && !c.isUnit ? emit(c) : undefined);
      const arms = [arm(ok), arm(err)];
      const writes = arms.map((c) => (c ? `${c}.write(v.val,w)` : 'undefined'));
      const reads = arms.map((c) =>
        c
          ? `${c}.read(r,n.val.val)`
          : '(n.val.val===undefined?undefined:(()=>{throw new TypeError("unexpected result payload")})())',
      );
      write = `if(!v||(v.tag!=="ok"&&v.tag!=="err"))throw new TypeError("expected result");if((v.tag==="ok"?${!arms[0]}:${!arms[1]})&&v.val!==undefined)throw new TypeError("unexpected result payload");return w.add({tag:"result-value",val:{tag:v.tag==="ok"?"ok-value":"err-value",val:v.tag==="ok"?${writes[0]}:${writes[1]}}});`;
      read = `if(n.val.tag!=="ok-value"&&n.val.tag!=="err-value")throw new TypeError("unknown result arm");return n.val.tag==="ok-value"?{tag:"ok",val:${reads[0]}}:{tag:"err",val:${reads[1]}};`;
    } else if (body.tag === 'map') {
      const k = emit(codec.mapKey ?? child(body.key));
      const v = emit(codec.mapValue ?? child(body.value));
      const object =
        codec.sourceSchema?._def?.type === 'record' ||
        codec.sourceSchema?._def?.typeName === 'ZodRecord';
      write = `if(${object ? 'typeof v!=="object"||v===null||Array.isArray(v)' : '!(v instanceof Map)'})throw new TypeError("expected map");return w.add({tag:"map-value",val:[...${object ? 'Object.entries(v)' : 'v'}].map(([k,v])=>({key:${k}.write(k,w),value:${v}.write(v,w)}))});`;
      read = `const entries=n.val.map(e=>[${k}.read(r,e.key),${v}.read(r,e.value)]);return ${object ? 'Object.fromEntries(entries)' : 'new Map(entries)'};`;
    } else if (['secret', 'quota-token', 'permission-card'].includes(body.tag)) {
      tag = body.tag === 'secret' ? 'secret-value' : `${body.tag}-handle`;
      write = `return w.resource(${literal(tag)},v);`;
      read = 'return r.resource(n);';
    } else if (body.tag === 'stream') {
      const item = emit(codec.streamItem ?? child(body.element));
      write = `return w.stream(v,${item});`;
      read = `return r.stream(n,${item});`;
    } else if (body.tag === 'variant') {
      const meta = codec.concrete;
      if (meta?.tag === 'unstructured-text' || meta?.tag === 'unstructured-binary') {
        const binary = meta.tag === 'unstructured-binary';
        const allowed =
          body.cases[0].payload.body.restrictions?.[binary ? 'mimeTypes' : 'languages'];
        const check = (expr) =>
          allowed?.length
            ? `if(${expr}!==undefined&&!${literal(allowed)}.includes(${expr}))throw new TypeError("unstructured restriction mismatch");`
            : '';
        write = `if(v?.tag==="url"){if(typeof v.val!=="string")throw new TypeError("invalid URL");return w.add({tag:"variant-value",val:{case_:1,payload:w.add({tag:"url-value",val:v.val})}});}if(v?.tag!=="inline"${binary ? '||!(v.val instanceof Uint8Array)' : '||typeof v.val!=="string"'})throw new TypeError("invalid unstructured value");${check(binary ? 'v.mimeType' : 'v.languageCode')}return w.add({tag:"variant-value",val:{case_:0,payload:w.add({tag:${binary ? '"binary-value"' : '"text-value"'},val:{${binary ? 'bytes:v.val,mimeType:v.mimeType' : 'text:v.val,language:v.languageCode'}}})}});`;
        read = `if(n.val.case_===1)return {tag:"url",val:r.node(n.val.payload,"url-value",x=>{if(typeof x.val!=="string")throw new TypeError("invalid URL");return x.val;})};if(n.val.case_!==0)throw new TypeError("unknown unstructured case");return r.node(n.val.payload,${binary ? '"binary-value"' : '"text-value"'},x=>{${check(binary ? 'x.val.mimeType' : 'x.val.language')}return {tag:"inline",val:x.val.${binary ? 'bytes' : 'text'},...(${binary ? 'x.val.mimeType===undefined?{}:{mimeType:x.val.mimeType}' : 'x.val.language===undefined?{}:{languageCode:x.val.language}'})};});`;
      } else if (meta?.tag === 'principal') {
        const cases = body.cases.map((entry) => entry.payload && emit(child(entry.payload)));
        write = `const names=["oidc","agent","golem-user","anonymous"];const i=names.indexOf(v?.tag);if(i<0)throw new TypeError("invalid principal");return w.add({tag:"variant-value",val:{case_:i,payload:i===3?undefined:[${cases.join(',')}][i].write(v.inner,w)}});`;
        read = `const i=n.val.case_;if(!Number.isInteger(i)||i<0||i>3)throw new TypeError("invalid principal");return __concretePrincipal.fromHost(i===3?{tag:"anonymous"}:{tag:["oidc","agent","golem-user"][i],val:[${cases.join(',')}][i].read(r,n.val.payload)});`;
      } else {
        const options = meta?.tag === 'variant' ? meta.cases : undefined;
        if (!Array.isArray(options))
          throw new Error('Static variants require explicit source alternatives');
        const cases = options.map((option) => option.codec && emit(option.codec));
        const payload = (c, value = 'v') => (c ? `${c}.write(${value},w)` : 'undefined');
        if (meta.sourceTag) {
          write = `const i=${literal(options.map((c) => c.value))}.indexOf(v?.[${literal(meta.sourceTag)}]);switch(i){${cases.map((c, i) => `case ${i}:{const {${meta.sourceTag}:_,...p}=v;return w.add({tag:"variant-value",val:{case_:${i},payload:${payload(c, 'p')}}});}`).join('')}default:throw new TypeError("unknown variant case");}`;
          read = `switch(n.val.case_){${cases.map((c, i) => `case ${i}:return {${literal(meta.sourceTag)}:${literal(options[i].value)}${c ? `,...${c}.read(r,n.val.payload)` : ''}};`).join('')}default:throw new TypeError("unknown variant case");}`;
        } else if (meta.discriminator) {
          write = `switch(v?.[${literal(meta.discriminator)}]){${cases.map((c, i) => `case ${literal(options[i].value)}:return w.add({tag:"variant-value",val:{case_:${i},payload:${payload(c)}}});`).join('')}default:throw new TypeError("unknown variant case");}`;
          read = `switch(n.val.case_){${cases.map((c, i) => `case ${i}:return ${c}.read(r,n.val.payload);`).join('')}default:throw new TypeError("unknown variant case");}`;
        } else {
          write = `${cases.map((c, i) => `{const result=w.trial(()=>${payload(c)});if(result!==undefined)return w.add({tag:"variant-value",val:{case_:${i},payload:result}});}`).join('')}throw new TypeError("no matching variant case");`;
          read = `switch(n.val.case_){${cases.map((c, i) => `case ${i}:return ${c}.read(r,n.val.payload);`).join('')}default:throw new TypeError("unknown variant case");}`;
        }
      }
    } else if (body.tag === 'enum') {
      const cases = literal(body.cases);
      write = `const i=${cases}.indexOf(v);if(i<0)throw new TypeError("unknown enum case");return w.add({tag:"enum-value",val:i});`;
      read = `if(!Number.isInteger(n.val)||n.val<0||n.val>=${body.cases.length})throw new TypeError("unknown enum case");return ${cases}[n.val];`;
    } else if (body.tag === 'datetime') {
      write = `if(!v||typeof v.seconds!=="bigint"||!Number.isInteger(v.nanoseconds)||v.nanoseconds<0||v.nanoseconds>=1000000000)throw new TypeError("invalid datetime");return w.add({tag:"datetime-value",val:v});`;
      read = `const v=n.val;if(!v||typeof v.seconds!=="bigint"||!Number.isInteger(v.nanoseconds)||v.nanoseconds<0||v.nanoseconds>=1000000000)throw new TypeError("invalid datetime");return v;`;
    } else if (body.tag === 'duration') {
      write = `if(typeof v!=="bigint")throw new TypeError("invalid duration");return w.add({tag:"duration-value",val:{nanoseconds:v}});`;
      read = `if(!n.val||typeof n.val.nanoseconds!=="bigint")throw new TypeError("invalid duration");return n.val.nanoseconds;`;
    } else if (body.tag === 'url' || body.tag === 'path') {
      write = `if(typeof v!=="string")throw new TypeError("invalid ${body.tag}");return w.add({tag:${literal(tag)},val:v});`;
      read = `if(typeof n.val!=="string")throw new TypeError("invalid ${body.tag}");return n.val;`;
    } else if (body.tag === 'text') {
      write = `if(!v||typeof v.text!=="string")throw new TypeError("invalid text");return w.add({tag:"text-value",val:v});`;
      read = `if(!n.val||typeof n.val.text!=="string")throw new TypeError("invalid text");return n.val;`;
    } else if (body.tag === 'binary') {
      write = `if(!v||!(v.bytes instanceof Uint8Array))throw new TypeError("invalid binary");return w.add({tag:"binary-value",val:v});`;
      read = `if(!n.val||!(n.val.bytes instanceof Uint8Array))throw new TypeError("invalid binary");return n.val;`;
    } else if (
      [
        'string',
        'char',
        'bool',
        's8',
        's16',
        's32',
        's64',
        'u8',
        'u16',
        'u32',
        'u64',
        'f32',
        'f64',
      ].includes(body.tag)
    ) {
      const t = body.tag;
      const integer = /^([su])(8|16|32|64)$/.exec(t);
      let valid =
        t === 'bool'
          ? 'typeof v==="boolean"'
          : ['string', 'char'].includes(t)
            ? 'typeof v==="string"'
            : 'typeof v==="number"';
      if (integer) {
        const bits = BigInt(integer[2]);
        const min = integer[1] === 's' ? -(2n ** (bits - 1n)) : 0n;
        const max = 2n ** (bits - (integer[1] === 's' ? 1n : 0n)) - 1n;
        valid = `${integer[2] === '64' ? 'typeof v==="bigint"' : 'typeof v==="number"&&Number.isInteger(v)'}&&v>=${literal(integer[2] === '64' ? min : Number(min))}&&v<=${literal(integer[2] === '64' ? max : Number(max))}`;
      }
      if (t === 'char')
        valid += '&&[...v].length===1&&!(v.codePointAt(0)>=0xd800&&v.codePointAt(0)<=0xdfff)';
      const source = codec.sourceSchema?._def;
      if (source?.type === 'literal' || source?.typeName === 'ZodLiteral')
        valid += `&&${literal(source.values ?? [source.value])}.includes(v)`;
      for (const [bound, op] of [
        ['min', '>='],
        ['max', '<='],
      ]) {
        const value = body.restrictions?.[bound];
        if (value) {
          let number = value.val;
          if (value.tag === 'float-bits') {
            const bytes = new DataView(new ArrayBuffer(8));
            bytes.setBigUint64(0, number, true);
            number = bytes.getFloat64(0, true);
          }
          valid += `&&v${op}${literal(number)}`;
        }
      }
      const check = `if(!(${valid}))throw new TypeError("invalid ${t}");`;
      write = `${check}return w.add({tag:${literal(tag)},val:${t === 'f32' ? 'Math.fround(v)' : 'v'}});`;
      read = `const v=n.val;${check}return v;`;
    } else throw new Error(`No static concrete codec emitter for ${body.tag}`);
    declarations[slot] =
      `const ${name}={read(r,i){return r.node(i,${literal(tag)},n=>{${read}})},write(v,w){${write}}};`;
    return name;
  }

  function emitDefinition(builder, clientOptions) {
    const tool = metadata().tool.getExtendedToolDefinition(builder);
    const descriptor = metadata().encode(tool);
    const declarations = [];
    const commands = [];
    const visit = (node, commandPath, aliases) => {
      if (node.body) {
        const fields = tool.canonicalInputFields(node);
        const inputGraph = tool.canonicalInputModel(node).codec.graph;
        const input = codecSource(
          {
            graph: inputGraph,
            fields: fields.map((f) => ({
              name: f.name.replace(/-([a-z0-9])/g, (_, c) => c.toUpperCase()),
              codec: f.codec,
            })),
          },
          declarations,
        );
        const typed = (codec) =>
          codec
            ? `{codec:${codecSource(codec, declarations)},graph:${literal(metadata().graph(codec.graph))}}`
            : 'undefined';
        commands.push(
          `{path:${literal(commandPath)},aliases:${literal(aliases)},nested:${node.subcommands.length > 0},input:{codec:${input},graph:${literal(metadata().graph(inputGraph))}},result:${typed(node.body.result?.codec)},errors:{${node.body.errors.map((e) => `${literal(e.name)}:${typed(e.payloadCodec)}`).join(',')}},stdin:${literal(node.body.stdin)},stdout:${literal(node.body.stdout)}}`,
        );
      }
      for (const child of node.subcommands)
        visit(
          child,
          [...commandPath, child.name],
          aliases.flatMap((p) => [child.name, ...child.aliases].map((n) => [...p, n])),
        );
    };
    visit(tool.root, [], [[]]);
    return `(()=>{${declarations.join('\n')}return ${clientOptions === undefined ? `__compiledTool(${literal(tool.toolName)},${literal(descriptor)},[${commands.join(',')}])` : `__compiledToolClient(${literal(tool.toolName)},[${commands.join(',')}],${clientOptions})`}})()`;
  }

  function emitAgent(definition) {
    const spec = definition.spec;
    if (!spec) throw new Error('Agent definitions must be statically evaluable');
    const reg = metadata().runtime.registerAgentType(spec.name, spec.id, spec.methods, {
      ...spec,
      dependencies: (spec.dependencies ?? []).map((d) => d.name),
    });
    const declarations = [];
    const input = (fields) => {
      const supplied = fields.filter((f) => f.codec.autoInjected !== 'principal');
      const graph = {
        defs: metadata().schema.mergeGraphDefs(supplied.map((f) => f.codec.graph)),
        root: metadata().schema.t.record(
          supplied.map((f) => metadata().schema.field(f.name, f.codec.graph.root)),
        ),
      };
      const codec = codecSource({ graph, fields: supplied }, declarations);
      return `{codec:${codec},principals:${literal(fields.filter((f) => f.codec.autoInjected === 'principal').map((f) => f.name))},hasInput:${fields.length !== 0},hasCallerInput:${supplied.length !== 0}}`;
    };
    const id = input(reg.idCodecs);
    const methods = [...reg.methodCodecs.values()].map(
      (method) =>
        `{name:${literal(method.name)},input:${input(method.inputCodecs)},output:${method.output.tag === 'unit' ? 'undefined' : codecSource(method.output.codec, declarations)}}`,
    );
    const snapshot = reg.snapshotStateSchema
      ? schemaExpressions.get(reg.snapshotStateSchema)
      : 'undefined';
    if (snapshot === undefined)
      throw new Error('Snapshot state schema has no static source expression');
    const config = (node) => {
      if (node.kind === 'group')
        return `{name:${literal(node.name)},requiredKeys:${literal(node.requiredKeys)},children:[${node.children.map(config).join(',')}]}`;
      const d = node.decl;
      const secret = d.source === 'secret';
      return `{name:${literal(d.name)},path:${literal(d.path)},graph:${literal(metadata().graph(d.graph))},codec:${codecSource(secret ? { graph: d.graph } : d.codec, declarations)},secret:${secret ? `{codec:${codecSource(d.codec, declarations)},graph:${literal(metadata().graph(d.codec.graph))}}` : 'undefined'}}`;
    };
    const configNodes = reg.configTree.children.map(config);
    return `(()=>{${declarations.join('\n')}return __compiledAgent(${literal(reg.agentType)},${id},[${methods.join(',')}],${snapshot},${spec.snapshotting !== undefined && spec.snapshotting !== 'disabled'},[${configNodes.join(',')}])})()`;
  }

  return {
    usesDynamicModels,
    transform(code, id) {
      const source = program.getSourceFile(id);
      if (!source || source.isDeclarationFile) return;
      const edits = [];
      const visit = (node) => {
        if (ts.isCallExpression(node)) {
          const declaration = checker.getResolvedSignature(node)?.declaration;
          if (
            declaration?.name?.text === 'client' &&
            declaration.parent?.name?.text !== 'ParsedAgentId' &&
            declaration.getSourceFile().fileName.endsWith('/dist/index.d.mts')
          ) {
            edits.push([
              node.getStart(source),
              node.end,
              emitDefinition(
                evaluate(node.arguments[0]),
                node.arguments[1]?.getText(source) ?? '{}',
              ),
            ]);
            return;
          }
          if (
            declaration?.name?.text === 'defineAgent' &&
            declaration.getSourceFile().fileName.endsWith('/dist/index.d.mts')
          ) {
            edits.push([node.getStart(source), node.end, emitAgent(evaluate(node))]);
            return;
          }
        }
        const receiver =
          ts.isPropertyAccessExpression(node) && node.name.text === 'implement'
            ? node.expression
            : ts.isElementAccessExpression(node) &&
                ts.isStringLiteralLike(node.argumentExpression) &&
                node.argumentExpression.text === 'implement'
              ? node.expression
              : ts.isVariableDeclaration(node) &&
                  ts.isObjectBindingPattern(node.name) &&
                  node.name.elements.some((e) => (e.propertyName ?? e.name).text === 'implement')
                ? node.initializer
                : undefined;
        if (receiver) {
          const symbol = checker.getTypeAtLocation(receiver).getProperty('implement');
          if (symbol?.declarations?.some((d) => d.parent?.name?.text === 'CommandBuilder')) {
            edits.push([
              receiver.getStart(source),
              receiver.end,
              emitDefinition(evaluate(receiver)),
            ]);
            return;
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
      if (!edits.length) return;
      const annotate = (node) => {
        if (
          ts.isVariableDeclaration(node) &&
          ts.isIdentifier(node.name) &&
          node.initializer &&
          values.has(checker.getSymbolAtLocation(node.name)) &&
          !edits.some(([start, end]) => start < node.end && end > node.getStart(source))
        ) {
          // Only metadata actually evaluated by the compiler is known pure.
          edits.push([
            node.initializer.getStart(source),
            node.initializer.end,
            `/*@__PURE__*/(()=>(${node.initializer.getText(source)}))()`,
          ]);
          return;
        }
        ts.forEachChild(node, annotate);
      };
      annotate(source);
      // Metadata spans belong to the original source, even when a Rollup load
      // hook has already transpiled the input module.
      code = source.text;
      for (const [start, end, replacement] of edits.sort((a, b) => b[0] - a[0]))
        code = code.slice(0, start) + replacement + code.slice(end);
      return {
        code: ts.transpileModule(
          `import { compiledTool as __compiledTool } from ${literal(path.join(runtime, 'internal/tool/compiled.mjs'))};\nimport { compiledToolClient as __compiledToolClient } from ${literal(path.join(runtime, 'toolClient.mjs'))};\nimport { compiledAgent as __compiledAgent, concretePrincipal as __concretePrincipal } from ${literal(path.join(runtime, 'internal/compiledAgent.mjs'))};\n${code}`,
          {
            fileName: id,
            compilerOptions: { ...config.options, module: ts.ModuleKind.ESNext, noEmit: false },
          },
        ).outputText,
        map: null,
      };
    },
  };
}

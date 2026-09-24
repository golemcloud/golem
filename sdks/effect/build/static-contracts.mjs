import fs from "node:fs"
import path from "node:path"
import { createRequire } from "node:module"
import ts from "typescript"
import * as EffectRuntime from "effect"

const packageName = "@golemcloud/effect-golem"
const literal = (value) => {
  if (value === undefined) return "undefined"
  if (typeof value === "bigint") return `${value}n`
  if (typeof value === "number" && !Number.isFinite(value)) return String(value)
  if (Array.isArray(value)) return `[${value.map(literal).join(",")}]`
  if (value !== null && typeof value === "object")
    return `{${Object.entries(value)
      .map(([key, v]) => `${JSON.stringify(key)}:${literal(v)}`)
      .join(",")}}`
  return JSON.stringify(value)
}

// Only metadata expressions are evaluated. Handler bodies and module statements
// are never run, and host calls are unavailable in the compiler.
function metadataLoader(runtime) {
  const cache = new Map()
  const load = (file) => {
    if (cache.has(file)) return cache.get(file).exports
    const module = { exports: {} }
    cache.set(file, module)
    const source = file
      .replace(`${path.sep}dist${path.sep}src${path.sep}`, `${path.sep}src${path.sep}`)
      .replace(/\.js$/, ".ts")
    const input = fs.existsSync(source) ? source : file
    const code = ts.transpileModule(fs.readFileSync(input, "utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    }).outputText
    const require = (id) => {
      if (id === "effect") return EffectRuntime
      if (/^(golem:|wasi:|node:sqlite)/.test(id))
        return new Proxy(
          {},
          {
            get: (_, key) =>
              key === "__esModule"
                ? true
                : () => {
                    throw new Error(`Host operation ${id}.${String(key)} in metadata`)
                  },
          },
        )
      if (id.startsWith(".")) return load(path.resolve(path.dirname(file), id))
      return createRequire(file)(id)
    }
    new Function("require", "module", "exports", code)(require, module, module.exports)
    return module.exports
  }
  return (name) => load(path.join(runtime, name))
}

export function staticContracts(runtime, publicEntries) {
  const load = metadataLoader(runtime)
  const sdkModule = (name) => publicEntries.get(name) ?? name.slice(packageName.length + 1) + ".js"
  const sources = new Map()
  const checkers = new Map()
  const transformed = new Map()
  const getSource = (id) => {
    if (sources.has(id)) return sources.get(id)
    const program = ts.createProgram([id], {
      allowJs: true,
      noResolve: true,
      noLib: true,
      target: ts.ScriptTarget.ESNext,
      module: ts.ModuleKind.ESNext,
    })
    const source = program.getSourceFile(id)
    checkers.set(id, program.getTypeChecker())
    sources.set(id, source)
    return source
  }
  const declaration = (node) =>
    checkers.get(node.getSourceFile().fileName).getSymbolAtLocation(node)?.declarations?.[0]
  const sdkValue = (node) => {
    if (ts.isPropertyAccessExpression(node)) return sdkValue(node.expression)?.[node.name.text]
    if (!ts.isIdentifier(node)) return undefined
    const decl = declaration(node)
    if (!decl || !(ts.isImportSpecifier(decl) || ts.isNamespaceImport(decl))) return undefined
    let statement = decl
    while (!ts.isImportDeclaration(statement)) statement = statement.parent
    const module = statement.moduleSpecifier.text
    if (module !== packageName && !module.startsWith(packageName + "/")) return undefined
    const exports = load(sdkModule(module))
    return ts.isNamespaceImport(decl) ? exports : exports[(decl.propertyName ?? decl.name).text]
  }
  const toolExpression = (node, seen = new Set()) => {
    if (seen.has(node)) return false
    seen.add(node)
    if (ts.isCallExpression(node)) {
      if (sdkValue(node.expression) === load("internal/tool/model.js").toolDefinition) return true
      return (
        ts.isPropertyAccessExpression(node.expression) &&
        toolExpression(node.expression.expression, seen)
      )
    }
    if (ts.isIdentifier(node)) {
      const decl = declaration(node)
      return (
        decl &&
        ts.isVariableDeclaration(decl) &&
        decl.initializer &&
        toolExpression(decl.initializer, seen)
      )
    }
    return false
  }
  const sourceValue = new Map()
  const sourceAst = new Map()
  const sourceNodes = new Map()
  const bindings = new Map()
  const localModule = (source, name) => {
    const base = path.resolve(path.dirname(source.fileName), name)
    const file = [
      base,
      base.replace(/\.js$/, ".ts"),
      `${base}.ts`,
      `${base}.mjs`,
      path.join(base, "index.ts"),
    ].find((p) => fs.existsSync(p) && fs.statSync(p).isFile())
    if (!file) throw new Error(`Cannot resolve metadata module ${name}`)
    return getSource(file)
  }
  const exported = (source, name) => {
    for (const statement of source.statements) {
      if (ts.isVariableStatement(statement)) {
        const decl = statement.declarationList.declarations.find(
          (d) => ts.isIdentifier(d.name) && d.name.text === name,
        )
        if (decl) return evaluate(decl.name)
      }
      if (ts.isExportDeclaration(statement)) {
        if (statement.exportClause && ts.isNamedExports(statement.exportClause)) {
          const item = statement.exportClause.elements.find((e) => e.name.text === name)
          if (item)
            return statement.moduleSpecifier
              ? exported(
                  localModule(source, statement.moduleSpecifier.text),
                  (item.propertyName ?? item.name).text,
                )
              : evaluate(item.propertyName ?? item.name)
        } else if (statement.moduleSpecifier) {
          const value = exported(localModule(source, statement.moduleSpecifier.text), name)
          if (value !== undefined) return value
        }
      }
    }
    throw new Error(`Metadata export ${name} is not statically defined in ${source.fileName}`)
  }
  const evaluate = (node, locals = new Map()) => {
    const value = evaluateExpression(node, locals)
    if (value && typeof value === "object") sourceNodes.set(value, node)
    if (EffectRuntime.Schema.isSchema(value)) {
      const expr = node.getText(node.getSourceFile())
      sourceValue.set(value, expr)
      sourceAst.set(value.ast, expr)
    }
    return value
  }
  const evaluateExpression = (node, locals) => {
    if (
      ts.isParenthesizedExpression(node) ||
      ts.isAsExpression(node) ||
      ts.isSatisfiesExpression(node)
    )
      return evaluate(node.expression, locals)
    if (ts.isStringLiteralLike(node)) return node.text
    if (ts.isNumericLiteral(node)) return Number(node.text)
    if (node.kind === ts.SyntaxKind.TrueKeyword) return true
    if (node.kind === ts.SyntaxKind.FalseKeyword) return false
    if (node.kind === ts.SyntaxKind.NullKeyword) return null
    if (ts.isIdentifier(node)) {
      if (node.text === "undefined") return undefined
      if (locals.has(node.text)) return locals.get(node.text)
      const source = node.getSourceFile()
      const key = `${source.fileName}:${node.text}`
      if (bindings.has(key)) return bindings.get(key)
      for (const statement of source.statements) {
        if (ts.isImportDeclaration(statement)) {
          const clause = statement.importClause
          const name = statement.moduleSpecifier.text
          const imported = () => {
            if (name === "effect") return EffectRuntime
            if (name === packageName || name.startsWith(packageName + "/"))
              return load(sdkModule(name))
            if (name.startsWith("."))
              return new Proxy({}, { get: (_, key) => exported(localModule(source, name), key) })
            throw new Error(`Metadata import is not supported: ${name}`)
          }
          if (
            clause?.namedBindings &&
            ts.isNamespaceImport(clause.namedBindings) &&
            clause.namedBindings.name.text === node.text
          )
            return imported()
          if (clause?.namedBindings && ts.isNamedImports(clause.namedBindings)) {
            const entry = clause.namedBindings.elements.find((e) => e.name.text === node.text)
            if (entry) return imported()[(entry.propertyName ?? entry.name).text]
          }
        }
        if (ts.isVariableStatement(statement))
          for (const decl of statement.declarationList.declarations) {
            if (ts.isIdentifier(decl.name) && decl.name.text === node.text && decl.initializer) {
              const value = evaluate(decl.initializer, locals)
              bindings.set(key, value)
              return value
            }
          }
        if (
          ts.isClassDeclaration(statement) &&
          statement.name?.text === node.text &&
          statement.members.length === 0
        ) {
          const parent = statement.heritageClauses?.[0]?.types?.[0]?.expression
          if (parent) return evaluate(parent, locals)
        }
      }
    }
    if (ts.isObjectLiteralExpression(node)) {
      const value = {}
      for (const prop of node.properties) {
        if (ts.isSpreadAssignment(prop)) Object.assign(value, evaluate(prop.expression, locals))
        else if (ts.isPropertyAssignment(prop))
          value[prop.name.text] = evaluate(prop.initializer, locals)
        else if (ts.isShorthandPropertyAssignment(prop))
          value[prop.name.text] = evaluate(prop.name, locals)
        else throw new Error("Metadata requires explicit properties")
      }
      return value
    }
    if (ts.isArrayLiteralExpression(node)) return node.elements.map((e) => evaluate(e, locals))
    if (ts.isPropertyAccessExpression(node))
      return evaluate(node.expression, locals)[node.name.text]
    if (ts.isArrowFunction(node) && !ts.isBlock(node.body))
      return (...args) =>
        evaluate(
          node.body,
          new Map([...locals, ...node.parameters.map((p, i) => [p.name.text, args[i]])]),
        )
    if (ts.isCallExpression(node)) {
      const receiver = ts.isPropertyAccessExpression(node.expression)
        ? evaluate(node.expression.expression, locals)
        : undefined
      const fn = receiver ? receiver[node.expression.name.text] : evaluate(node.expression, locals)
      if (typeof fn !== "function") throw new Error("Metadata call is not statically resolved")
      const value = fn.apply(
        receiver,
        node.arguments.map((a) => evaluate(a, locals)),
      )
      if (EffectRuntime.Schema.isSchema(value))
        sourceValue.set(value, node.getText(node.getSourceFile()))
      return value
    }
    throw new Error(`Unsupported static metadata: ${node.getText(node.getSourceFile())}`)
  }

  const runtimeParam = (param) => {
    if (load("Unstructured.js").isElementSpec(param)) return EffectRuntime.Schema.Unknown
    if (load("Multimodal.js").isMultimodal(param))
      return EffectRuntime.Schema.Array(
        EffectRuntime.Schema.Union(
          Object.entries(param.shape).map(([name, member]) =>
            EffectRuntime.Schema.Struct({
              _tag: EffectRuntime.Schema.Literal(name),
              value: runtimeParam(member),
            }),
          ),
        ),
      )
    return param
  }

  // Boundary carriers contain dynamic model compilers. Generated contracts keep
  // only their domain schemas; the wire layout is emitted separately below.
  const runtimeExpression = (node) => {
    if (ts.isCallExpression(node) || ts.isIdentifier(node)) {
      let value
      try {
        value = evaluate(node)
      } catch {
        // Non-metadata expressions are preserved, never executed by the compiler.
      }
      if (load("Unstructured.js").isElementSpec(value)) return "__Schema.Unknown"
      if (load("Multimodal.js").isMultimodal(value)) {
        const members = Object.entries(value.shape).map(([name, member]) => {
          const source = sourceNodes.get(member)
          const expression = source ? runtimeExpression(source) : sourceValue.get(member)
          if (!expression) throw new Error(`Missing multimodal member source: ${name}`)
          return `__Schema.Struct({_tag:__Schema.Literal(${literal(name)}),value:${expression}})`
        })
        return `__Schema.Array(__Schema.Union([${members.join(",")}]))`
      }
      if (ts.isIdentifier(node) && value && typeof value === "object" && !value.ast) {
        const decl = declaration(node)
        if (decl && ts.isVariableDeclaration(decl) && decl.initializer)
          return runtimeExpression(decl.initializer)
      }
    }
    const source = node.getSourceFile()
    let text = node.getText(source)
    const edits = []
    ts.forEachChild(node, (child) => {
      const replacement = runtimeExpression(child)
      if (replacement !== child.getText(source))
        edits.push([
          child.getStart(source) - node.getStart(source),
          child.end - node.getStart(source),
          replacement,
        ])
    })
    for (const [start, end, replacement] of edits.reverse())
      text = text.slice(0, start) + replacement + text.slice(end)
    return text
  }

  function emitCodec(
    graph,
    declarations,
    seen = new Map(),
    type = graph.root,
    rawAst,
    rawExpr,
    multimodal = false,
    optionalField = false,
  ) {
    if (seen.has(type)) return seen.get(type)
    const name = `codec${declarations.length}`
    const slot = declarations.push("") - 1
    seen.set(type, name)
    const body = type.body
    let ast = rawAst && EffectRuntime.SchemaAST.toEncoded(rawAst)
    let expr = rawExpr && `__SchemaAST.toEncoded(${rawExpr})`
    while (ast?._tag === "Suspend") {
      ast = EffectRuntime.SchemaAST.toEncoded(ast.thunk())
      expr = expr && `__SchemaAST.toEncoded(${expr}.thunk())`
    }
    const annotation = (key) => ast && EffectRuntime.SchemaAST.resolveAt(key)(ast)
    const native = annotation("effect-golem/schemaNode")
    const child = (t, a, e, m = false, o = false) =>
      emitCodec(graph, declarations, seen, t, a, e, m, o)
    if (body.tag === "ref") {
      const c = child(graph.defs.get(body.id).body, ast, expr)
      declarations[slot] =
        `const ${name}={read:(r,i)=>${c}.read(r,i),write:(v,w)=>${c}.write(v,w)};`
      return name
    }
    let tag = `${body.tag}-value`,
      read,
      write
    if (body.tag === "record" || body.tag === "tuple") {
      const fields =
        body.tag === "record"
          ? body.fields.map((f) => ({ name: f.name, body: f.body }))
          : body.elements.map((body, name) => ({ name, body }))
      const optional = fields.map((f) => {
        const type = ast?.propertySignatures?.find((p) => p.name === f.name)?.type
        return type !== undefined && EffectRuntime.SchemaAST.isOptional(type)
      })
      const children = fields.map((f, i) =>
        child(
          f.body,
          body.tag === "record"
            ? ast?.propertySignatures?.find((p) => p.name === f.name)?.type
            : ast?.elements?.[i],
          expr &&
            (body.tag === "record"
              ? `${expr}.propertySignatures[${ast.propertySignatures.findIndex((p) => p.name === f.name)}].type`
              : `${expr}.elements[${i}]`),
          false,
          optional[i],
        ),
      )
      write = `return w.add({tag:${literal(tag)},val:[${fields.map((f, i) => `${children[i]}.write(v[${literal(f.name)}],w)`).join(",")}]});`
      read = `if(!Array.isArray(n.val)||n.val.length!==${fields.length})throw new TypeError("wrong field count");return ${body.tag === "record" ? "{" : "["}${fields.map((f, i) => (optional[i] ? `...((v)=>v===undefined?{}:{${literal(f.name)}:v})(${children[i]}.read(r,n.val[${i}]))` : `${body.tag === "record" ? literal(f.name) + ":" : ""}${children[i]}.read(r,n.val[${i}])`)).join(",")}${body.tag === "record" ? "}" : "]"};`
    } else if (body.tag === "list" || body.tag === "fixed-list") {
      const representation = ast?.annotations?.representation?.id
      const mapKind =
        representation === "effect/schema/HashMap"
          ? "effect/HashMap"
          : ast?.annotations?.typeConstructor?._tag
      const map = ["ReadonlyMap", "effect/HashMap"].includes(mapKind)
      const typedArray = {
        u8: "Uint8Array",
        i8: "Int8Array",
        u16: "Uint16Array",
        i16: "Int16Array",
        u32: "Uint32Array",
        i32: "Int32Array",
        f32: "Float32Array",
        f64: "Float64Array",
        "big-i64": "BigInt64Array",
        "big-u64": "BigUint64Array",
      }[annotation("effect-golem/witTypedArray")]
      const elementAst = map
        ? EffectRuntime.Schema.Tuple(ast.typeParameters.map(EffectRuntime.Schema.make)).ast
        : (ast?.rest?.[0] ?? ast?.typeParameters?.[0])
      const c = child(
        body.element,
        elementAst,
        expr &&
          (map
            ? `__Schema.Tuple(${expr}.typeParameters.map(__Schema.make)).ast`
            : `${expr}.${ast.rest ? "rest" : "typeParameters"}[0]`),
        type.metadata?.role?.tag === "multimodal",
      )
      write = `return w.add({tag:${literal(tag)},val:Array.from(v,x=>${c}.write(x,w))});`
      const result = `n.val.map(i=>${c}.read(r,i))`
      read = `if(!Array.isArray(n.val)${body.tag === "fixed-list" ? `||n.val.length!==${body.length}` : ""})throw new TypeError("invalid list");return ${typedArray ? `new ${typedArray}(${result})` : map ? `${mapKind === "ReadonlyMap" ? "new Map" : "__HashMap.fromIterable"}(${result})` : result};`
    } else if (body.tag === "map") {
      const key = child(body.key, ast?.typeParameters?.[0], expr && `${expr}.typeParameters[0]`)
      const value = child(body.value, ast?.typeParameters?.[1], expr && `${expr}.typeParameters[1]`)
      write = `return w.add({tag:"map-value",val:Array.from(v,([k,v])=>({key:${key}.write(k,w),value:${value}.write(v,w)}))});`
      read = `if(!Array.isArray(n.val))throw new TypeError("invalid map");return new Map(n.val.map(e=>[${key}.read(r,e.key),${value}.read(r,e.value)]));`
    } else if (body.tag === "option") {
      const emptyAst = ast?.types?.find((a) => ["Null", "Undefined", "Void"].includes(a._tag))
      const effectOption = !optionalField && ast?._tag === "Declaration"
      const empty = effectOption
        ? "__Option.none()"
        : !optionalField && emptyAst?._tag === "Null"
          ? "null"
          : "undefined"
      const c = child(
        body.element,
        optionalField ? ast : (ast?.typeParameters?.[0] ?? ast?.types?.find((a) => a !== emptyAst)),
        expr &&
          (optionalField
            ? expr
            : effectOption
              ? `${expr}.typeParameters[0]`
              : `${expr}.types[${ast.types.findIndex((a) => a !== emptyAst)}]`),
      )
      write = `return w.add({tag:"option-value",val:${effectOption ? "__Option.isNone(v)" : `v===${empty}`}?undefined:${c}.write(${effectOption ? "v.value" : "v"},w)});`
      read = `return n.val===undefined?${empty}:${effectOption ? "__Option.some(" : ""}${c}.read(r,n.val)${effectOption ? ")" : ""};`
    } else if (body.tag === "result") {
      const ok =
        body.ok && child(body.ok, ast?.typeParameters?.[0], expr && `${expr}.typeParameters[0]`)
      const err =
        body.err && child(body.err, ast?.typeParameters?.[1], expr && `${expr}.typeParameters[1]`)
      write = `return w.add({tag:"result-value",val:__Result.isSuccess(v)?{tag:"ok-value",val:${ok ? `${ok}.write(v.success,w)` : "undefined"}}:{tag:"err-value",val:${err ? `${err}.write(v.failure,w)` : "undefined"}}});`
      read = `switch(n.val.tag){case "ok-value":return __Result.succeed(${ok ? `${ok}.read(r,n.val.val)` : "undefined"});case "err-value":return __Result.fail(${err ? `${err}.read(r,n.val.val)` : "undefined"});default:throw new TypeError("unknown result arm")}`
    } else if (
      body.tag === "variant" &&
      ["unstructured-text", "unstructured-binary"].includes(type.metadata?.role?.tag)
    ) {
      const binary = type.metadata.role.tag === "unstructured-binary"
      const payload = binary ? "binary" : "text"
      const field = binary ? "bytes" : "text"
      const restriction = binary ? "mimeType" : "language"
      const domain = binary ? "mimeType" : "languageCode"
      const allowed =
        body.cases[0].payload.body.restrictions?.[binary ? "mimeTypes" : "languages"] ?? []
      const check = `if(typeof v.val!==${literal(binary ? "object" : "string")}${binary ? "||!(v.val instanceof Uint8Array)" : ""})throw new TypeError("invalid inline payload");if(v.${domain}!==undefined&&typeof v.${domain}!=="string")throw new TypeError("invalid restriction");${allowed.length ? `if(v.${domain}&& !${literal(allowed)}.includes(v.${domain}))throw new TypeError("disallowed ${restriction}");` : ""}`
      read = `switch(n.val.case_){case 0:return r.node(n.val.payload,${literal(payload + "-value")},p=>{const v={_tag:"inline",val:p.val.${field},${domain}:p.val.${restriction}};${check}return v;});case 1:return r.node(n.val.payload,"url-value",p=>{if(typeof p.val!=="string")throw new TypeError("invalid URL");return {_tag:"url",val:p.val};});default:throw new TypeError("unknown unstructured case")}`
      write = `switch(v._tag){case "inline":{${check}return w.add({tag:"variant-value",val:{case_:0,payload:w.add({tag:${literal(payload + "-value")},val:{${field}:v.val,${restriction}:v.${domain}}})}});}case "url":if(typeof v.val!=="string")throw new TypeError("invalid URL");return w.add({tag:"variant-value",val:{case_:1,payload:w.add({tag:"url-value",val:v.val})}});default:throw new TypeError("unknown unstructured case")}`
    } else if (body.tag === "variant" && annotation("effect-golem/witPrincipal")) {
      const cases = body.cases.map((c) => c.payload && child(c.payload))
      read = `switch(n.val.case_){${body.cases.map((c, i) => `case ${i}:${cases[i] ? `return {tag:${literal(c.name)},val:${cases[i]}.read(r,n.val.payload)};` : `if(n.val.payload!==undefined)throw new TypeError("unexpected principal payload");return {tag:${literal(c.name)}};`}`).join("")}default:throw new TypeError("unknown principal case")}`
      write = `switch(v.tag){${body.cases.map((c, i) => `case ${literal(c.name)}:return w.add({tag:"variant-value",val:{case_:${i},payload:${cases[i] ? `${cases[i]}.write(v.val,w)` : "undefined"}}});`).join("")}default:throw new TypeError("unknown principal case")}`
    } else if (body.tag === "variant") {
      const tagged = ast?.types?.every(
        (a) => a._tag === "Objects" && a.propertySignatures.some((p) => p.name === "_tag"),
      )
      if (!ast?.types) throw new Error("Variant requires concrete source alternatives")
      const cases = body.cases.map(
        (c, i) =>
          c.payload &&
          child(
            c.payload,
            multimodal
              ? ast.types[i].propertySignatures.find((p) => p.name === "value").type
              : ast.types[i],
            expr &&
              (multimodal
                ? `${expr}.types[${i}].propertySignatures.find(p=>p.name==="value").type`
                : `${expr}.types[${i}]`),
          ),
      )
      read = `switch(n.val.case_){${body.cases.map((c, i) => `case ${i}:${cases[i] ? `return ${tagged ? `{_tag:${literal(c.name)},${multimodal ? "value:" : "..."}` : ""}${cases[i]}.read(r,n.val.payload)${tagged ? "}" : ""};` : `if(n.val.payload!==undefined)throw new TypeError("unexpected variant payload");return ${tagged ? `{_tag:${literal(c.name)}}` : ast.types[i]._tag === "Null" ? "null" : "undefined"};`}`).join("")}default:throw new TypeError("unknown variant case")}`
      write = tagged
        ? `switch(v._tag){${body.cases.map((c, i) => `case ${literal(c.name)}:return w.add({tag:"variant-value",val:{case_:${i},payload:${cases[i] ? `${cases[i]}.write(${multimodal ? "v.value" : "v"},w)` : "undefined"}}});`).join("")}default:throw new TypeError("unknown variant case")}`
        : `${cases.map((c, i) => (c ? `{const i=w.trial(()=>${c}.write(v,w));if(i!==undefined)return w.add({tag:"variant-value",val:{case_:${i},payload:i}});}` : `if(v===${ast.types[i]._tag === "Null" ? "null" : "undefined"})return w.add({tag:"variant-value",val:{case_:${i}}});`)).join("")}throw new TypeError("no matching variant case");`
    } else if (body.tag === "enum") {
      read = `if(!Number.isInteger(n.val)||n.val<0||n.val>=${body.cases.length})throw new TypeError("unknown enum case");return ${literal(body.cases)}[n.val];`
      write = `const i=${literal(body.cases)}.indexOf(v);if(i<0)throw new TypeError("unknown enum case");return w.add({tag:"enum-value",val:i});`
    } else if (["secret", "quota-token", "permission-card"].includes(body.tag)) {
      tag = body.tag === "secret" ? "secret-value" : `${body.tag}-handle`
      read = "return r.resource(n);"
      write = `return w.resource(${literal(tag)},v);`
    } else if (body.tag === "stream") {
      const itemAst = native?.elementSchema?.ast ?? ast?.typeParameters?.[0]
      const schema = expr
        ? native?.elementSchema
          ? `__SchemaAST.resolveAt("effect-golem/schemaNode")(${expr}).elementSchema`
          : `__Schema.make(${expr}.typeParameters[0])`
        : (sourceValue.get(native?.elementSchema) ?? sourceAst.get(itemAst))
      if (!schema) throw new Error("Stream item schema has no static source expression")
      const c = child(body.element, itemAst, `${schema}.ast`)
      const item = `{...${c},schema:${schema}}`
      read = `return r.stream(n,${item});`
      write = `return w.stream(v,${item});`
    } else if (body.tag === "flags") {
      const valid = `Array.isArray(v)&&v.length===${body.names.length}&&v.every(x=>typeof x==="boolean")`
      read = `const v=n.val;if(!(${valid}))throw new TypeError("invalid flags");return v;`
      write = `if(!(${valid}))throw new TypeError("invalid flags");return w.add({tag:"flags-value",val:v});`
    } else if (["text", "binary", "duration"].includes(body.tag)) {
      const field = { text: "text", binary: "bytes", duration: "nanoseconds" }[body.tag]
      read = `return n.val.${field};`
      write = `return w.add({tag:${literal(tag)},val:{${field}:v}});`
    } else if (["path", "url", "datetime", "quantity"].includes(body.tag)) {
      read = "return n.val;"
      write = `return w.add({tag:${literal(tag)},val:v});`
    } else if (
      [
        "string",
        "char",
        "bool",
        "u8",
        "u16",
        "u32",
        "u64",
        "s8",
        "s16",
        "s32",
        "s64",
        "f32",
        "f64",
      ].includes(body.tag)
    ) {
      const integer = /^([su])(8|16|32|64)$/.exec(body.tag)
      let valid = `typeof v===${literal(body.tag === "bool" ? "boolean" : ["string", "char"].includes(body.tag) ? "string" : body.tag.endsWith("64") && integer ? "bigint" : "number")}`
      if (integer) {
        const bits = BigInt(integer[2]),
          signed = integer[1] === "s"
        const min = signed ? -(2n ** (bits - 1n)) : 0n,
          max = 2n ** (bits - (signed ? 1n : 0n)) - 1n
        valid += `${bits === 64n ? "" : "&&Number.isInteger(v)"}&&v>=${literal(bits === 64n ? min : Number(min))}&&v<=${literal(bits === 64n ? max : Number(max))}`
      }
      if (body.tag === "char")
        valid += "&&[...v].length===1&&!(v.codePointAt(0)>=0xd800&&v.codePointAt(0)<=0xdfff)"
      if (ast?._tag === "Literal") valid += `&&v===${literal(ast.literal)}`
      const check = `if(!(${valid}))throw new TypeError(${literal("invalid " + body.tag)});`
      read = `const v=n.val;${check}return v;`
      write = `${check}return w.add({tag:${literal(tag)},val:${body.tag === "f32" ? "Math.fround(v)" : "v"}});`
    } else throw new Error(`Concrete codec emission is not supported for ${body.tag}`)
    declarations[slot] =
      `const ${name}={read(r,i){return r.node(i,${literal(tag)},n=>{${read}})},write(v,w){${write}}};`
    return name
  }

  function emitGraphCheck(graph, declarations) {
    const prefix = `graph${declarations.length}`
    const compare = (value, expr) => {
      if (Array.isArray(value))
        return `Array.isArray(${expr})&&${expr}.length===${value.length}${value.map((v, i) => `&&(${compare(v, `${expr}[${i}]`)})`).join("")}`
      if (value !== null && typeof value === "object")
        return `${expr}!=null${Object.entries(value)
          .filter(([k]) => k !== "metadata")
          .map(([k, v]) => `&&(${compare(v, `${expr}[${literal(k)}]`)})`)
          .join("")}`
      return `${expr}===${literal(value)}`
    }
    const node = (index, expr) =>
      index === undefined ? `${expr}===undefined` : `${prefix}_${index}(g,${expr},seen)`
    graph.typeNodes.forEach(({ body }, index) => {
      let check
      const v = body.val
      switch (body.tag) {
        case "ref-type":
          declarations.push(
            `function ${prefix}_${index}(g,i,seen){return ${node(graph.defs[v].body, "i")};}`,
          )
          return
        case "record-type":
        case "variant-type": {
          const field = body.tag === "record-type" ? "body" : "payload"
          check = `Array.isArray(b.val)&&b.val.length===${v.length}${v.map((f, i) => `&&b.val[${i}].name===${literal(f.name)}&&${node(f[field], `b.val[${i}].${field}`)}`).join("")}`
          break
        }
        case "tuple-type":
          check = `Array.isArray(b.val)&&b.val.length===${v.length}${v.map((n, i) => `&&${node(n, `b.val[${i}]`)}`).join("")}`
          break
        case "list-type":
        case "option-type":
        case "stream-type":
        case "future-type":
          check = node(v, "b.val")
          break
        case "fixed-list-type":
          check = `b.val?.length===${v.length}&&${node(v.element, "b.val.element")}`
          break
        case "map-type":
          check = `${node(v.key, "b.val?.key")}&&${node(v.value, "b.val?.value")}`
          break
        case "result-type":
          check = `${node(v.ok, "b.val?.ok")}&&${node(v.err, "b.val?.err")}`
          break
        case "secret-type":
          check = `b.val?.category===${literal(v.category)}&&${node(v.inner, "b.val?.inner")}`
          break
        default:
          check = compare(v, "b.val")
      }
      declarations.push(
        `function ${prefix}_${index}(g,i,seen){const key=${literal(String(index) + ":")}+i;if(seen.has(key))return true;seen.add(key);const refs=new Set();let b;for(;;){if(!Number.isInteger(i)||i<0||i>=g.typeNodes.length||refs.has(i))return false;refs.add(i);b=g.typeNodes[i].body;if(b.tag!=="ref-type")break;i=g.defs[b.val]?.body;}return b.tag===${literal(body.tag)}&&(${check});}`,
      )
    })
    return `g=>{if(!${prefix}_${graph.root}(g,g.root,new Set()))throw new TypeError("tool input schema does not match the command");}`
  }

  const run = EffectRuntime.Effect.runSync
  function compileTool(builder, clientOptions) {
    const model = load("internal/tool/model.js")
    const definition = { name: builder.model.name, model: builder.model }
    const compiled = model.compileDefinition(definition)
    const declarations = []
    const bodies = [...compiled.bodies].map(([key, body]) => {
      const inputAst = EffectRuntime.Schema.Struct(
        Object.fromEntries(body.args.map((a) => [a.name, a.schema])),
      ).ast
      const inputExpr = `__Schema.Struct(${literalFields(body.args)})`
      const input = `__wire(${inputExpr},${literal(body.input.schemaGraph)},${emitCodec(body.input.graph, declarations, new Map(), body.input.graph.root, inputAst, `${inputExpr}.ast`)})`
      const encode = (schema, codec) => {
        if (!schema) return "undefined"
        const wire = `__wire(${schemaSource(schema)},${literal(codec.schemaGraph)},${emitCodec(codec.graph, declarations, new Map(), codec.graph.root, schema.ast, `(${schemaSource(schema)}).ast`)})`
        if (clientOptions === undefined) return wire
        const check = emitGraphCheck(codec.schemaGraph, declarations)
        return `__Effect.succeed(((codec)=>({...codec,decodeTyped:v=>codec.decode(v.value,()=>(${check})(v.graph))}))(${wire}))`
      }
      if (clientOptions !== undefined)
        return `{path:${literal(key ? key.split("/") : [])},fields:${literal(body.args.map((a) => a.name))},stdout:${!!body.model.stdout},input:__Effect.succeed(${input}),output:${encode(body.model.output, body.output)},errors:[${body.errors.map(({ spec, codec }) => `{name:${literal(spec.name)},codec:${encode(spec.schema, codec)}}`).join(",")}]}`
      const check = emitGraphCheck(body.input.schemaGraph, declarations)
      return `[${literal(key)},{model:${literal({ stdin: body.model.stdin, stdout: body.model.stdout })},decodeInput:input=>${input}.decode(input.value,()=>(${check})(input.graph)),output:${encode(body.model.output, body.output)},errors:[${body.errors.map(({ spec, codec }) => `{spec:${literal({ name: spec.name })},codec:${encode(spec.schema, codec)}}`).join(",")}]}]`
    })
    if (clientOptions !== undefined)
      return `(()=>{${declarations.join("\n")}return __toolClient(${literal(definition.name)},[${bodies.join(",")}],${clientOptions})})()`
    return `(()=>{${declarations.join("\n")}return {implement:(impl,layer)=>__tool({definition:{name:${literal(definition.name)}},wire:${literal(compiled.wire)},bodies:new Map([${bodies.join(",")}])},impl,layer)}})()`
  }
  const schemaSource = (schema) => {
    const known = sourceValue.get(schema)
    if (known) return known
    for (const name of ["String", "Number", "Boolean", "BigInt", "Void", "Unknown"])
      if (EffectRuntime.Schema[name] === schema) return `__Schema.${name}`
    throw new Error("Schema has no static source expression")
  }
  const literalFields = (args) =>
    `{${args.map((a) => `${literal(a.name)}:${a.kind === "tail" || a.repeatable ? `__Schema.Array(${schemaSource(a.wireSchema)})` : a.flag === "count" ? "__Schema.Number" : schemaSource(a.schema)}`).join(",")}}`

  function compileConfig(name, fields, expression) {
    const compiled = run(load("Config.js").compileConfig(fields, name))
    const declarations = []
    const leaves = compiled.leaves.map((leaf) => {
      let schema = fields[leaf.path[0]]
      let expr = `fields[${literal(leaf.path[0])}]`
      for (const segment of leaf.path.slice(1)) {
        let ast = schema.ast
        let astExpr = `${expr}.ast`
        if (ast._tag === "Union") {
          const index = ast.types.findIndex((a) => a._tag === "Objects")
          ast = ast.types[index]
          astExpr += `.types[${index}]`
        }
        const index = ast.propertySignatures.findIndex((p) => p.name === segment)
        schema = EffectRuntime.Schema.make(ast.propertySignatures[index].type)
        expr = `__Schema.make(${astExpr}.propertySignatures[${index}].type)`
      }
      if (leaf.source === "secret") {
        schema = EffectRuntime.Schema.make(schema.ast.typeParameters[0])
        expr = `__Schema.make(${expr}.ast.typeParameters[0])`
      }
      if (leaf.schema.ast !== schema.ast) expr = `__Schema.UndefinedOr(${expr})`
      const codec = emitCodec(
        leaf.codec.graph,
        declarations,
        new Map(),
        leaf.codec.graph.root,
        leaf.schema.ast,
        `${expr}.ast`,
      )
      return `{source:${literal(leaf.source)},path:${literal(leaf.path)},declarationSchema:${literal(load("internal/schema-model/wit.js").schemaGraphToWit(leaf.declarationGraph))},codec:__wire(${expr},${literal(leaf.codec.schemaGraph)},${codec})}`
    })
    return `__config(${literal(name)},${expression},fields=>{${declarations.join("\n")}return __configRuntime([${leaves.join(",")}],new Set(${literal([...compiled.branches])}));})`
  }

  function compileAgent(spec, expression) {
    const agent = load("internal/agent.js")
    const method = load("internal/method.js")
    const snapshot = spec.snapshotting
      ? run(load("Snapshot.js").compileSnapshot(spec.name, spec.snapshotting))
      : null
    run(
      agent.registerAgent(spec, {
        init() {
          throw new Error("metadata only")
        },
        methods() {
          throw new Error("metadata only")
        },
      }),
    )
    const descriptor = agent.dispatchDiscoverAgentTypes().find((a) => a.typeName === spec.name)
    const declarations = []
    const input = (fields, expression) => {
      const codec = run(method.compileParamBindings(spec.name, fields))
      const runtimeFields = Object.fromEntries(
        Object.entries(fields).map(([name, param]) => [name, runtimeParam(param)]),
      )
      return `__wire(__Schema.Struct(${expression}),${literal(codec.schemaGraph)},${emitCodec(codec.graph, declarations, new Map(), codec.graph.root, EffectRuntime.Schema.Struct(runtimeFields).ast, `__Schema.Struct(${expression}).ast`)})`
    }
    const constructor = input(spec.id, "spec.id")
    const methods = Object.entries(spec.methods).map(([name, def]) => {
      const codec = run(method.compileMethodSpec(name, def))
      const success = codec.successVoid
        ? EffectRuntime.Schema.Struct({})
        : runtimeParam(def.success)
      const schema = codec.errorWrapped ? EffectRuntime.Schema.Result(success, def.error) : success
      const schemaExpr = codec.errorWrapped
        ? `__Schema.Result(${codec.successVoid ? "__Schema.Struct({})" : `spec.methods[${literal(name)}].success`},spec.methods[${literal(name)}].error)`
        : `spec.methods[${literal(name)}].success`
      const output = codec.outputCodec
        ? `__wire(${schemaExpr},${literal(codec.schemaGraph)},${emitCodec(codec.outputCodec.graph, declarations, new Map(), codec.outputCodec.graph.root, schema.ast, `(${schemaExpr}).ast`)})`
        : "undefined"
      const outputName = `output${declarations.length}`
      declarations.push(`const ${outputName}=${output};`)
      const streaming = codec.schemaGraph.typeNodes.some((n) => n.body.tag === "stream-type")
      return `[${literal(name)},{name:${literal(name)},inputCodec:${input(def.input, `spec.methods[${literal(name)}].input`)},encodeOutput:${outputName}?.encodeAsync,decodeOutput:${outputName}?.decode,streaming:${streaming},errorWrapped:${codec.errorWrapped},successVoid:${codec.successVoid},readOnly:${literal(codec.readOnly)}}]`
    })
    const cs = snapshot
      ? `{...${literal({ ...snapshot, schema: undefined })},schema:spec.snapshotting.schema}`
      : "null"
    const config = spec.config ? "spec.config.__wireConfig" : "null"
    const overrides = spec.config
      ? `,options=>options?.overrides===undefined?__Effect.succeed([]):__encodeOverrides(spec.config.__wireConfig,options.overrides)`
      : ""
    return `((spec)=>{spec=Object.freeze({...spec,id:Object.freeze({...spec.id}),methods:Object.freeze({...spec.methods})});${declarations.join("\n")}const constructorCodec=${constructor};const methods=new Map([${methods.join(",")}]);const client=__client(spec,__Effect.succeed({constructorCodec,methods})${overrides});const definition=Object.freeze({...spec,client,implement:(impl)=>{__agent({name:spec.name,metadata:spec,constructorCodec,methodCodecs:methods,agentType:${literal(descriptor)},compiledConfig:${config},compiledSnapshot:${cs}},impl);return Object.freeze({...spec,client,spec:definition})}});return definition})(${expression})`
  }

  return {
    name: "golem-effect-static-contracts",
    transform(code, id) {
      if (transformed.has(id)) return transformed.get(id)
      if (
        id.startsWith("\0") ||
        id.includes("/node_modules/") ||
        id.startsWith(runtime + path.sep) ||
        !/\.[cm]?[jt]s$/.test(id)
      )
        return null
      const source = getSource(id)
      if (
        !source.statements.some(
          (s) => ts.isImportDeclaration(s) && s.moduleSpecifier.text.startsWith(packageName),
        )
      )
        return null
      const edits = []
      const visit = (node) => {
        if (
          ts.isCallExpression(node) &&
          (ts.isIdentifier(node.expression) || ts.isPropertyAccessExpression(node.expression))
        ) {
          const constructor = sdkValue(node.expression)
          if (constructor !== undefined && constructor === load("Tool.js").client) {
            edits.push([
              node.getStart(source),
              node.end,
              compileTool(
                evaluate(node.arguments[0]),
                node.arguments[1]?.getText(source) ?? "undefined",
              ),
            ])
            return
          }
          if (constructor !== undefined && constructor === load("Config.js").defineConfig) {
            edits.push([
              node.getStart(source),
              node.end,
              compileConfig(
                evaluate(node.arguments[0]),
                evaluate(node.arguments[1]),
                node.arguments[1].getText(source),
              ),
            ])
            return
          }
          if (constructor !== undefined && constructor === load("internal/agent.js").defineAgent) {
            let outer = node
            while (
              outer.parent &&
              (ts.isCallExpression(outer.parent) || ts.isPropertyAccessExpression(outer.parent)) &&
              outer.parent.getStart(source) === node.getStart(source)
            )
              outer = outer.parent
            const prefix = ts.isExpressionStatement(outer.parent) ? ";" : ""
            edits.push([
              node.getStart(source),
              node.end,
              prefix +
                compileAgent(evaluate(node.arguments[0]), runtimeExpression(node.arguments[0])),
            ])
            return
          }
        }
        if (
          ts.isCallExpression(node) &&
          ts.isPropertyAccessExpression(node.expression) &&
          node.expression.name.text === "implement"
        ) {
          const receiver = node.expression.expression
          const prefix = ts.isExpressionStatement(node.parent) ? ";" : ""
          if (toolExpression(receiver)) {
            edits.push([
              receiver.getStart(source),
              receiver.end,
              prefix + compileTool(evaluate(receiver)),
            ])
            return
          }
        }
        ts.forEachChild(node, visit)
      }
      visit(source)
      if (!edits.length) return null
      const annotate = (node) => {
        if (edits.some(([start, end]) => node.getStart(source) >= start && node.end <= end)) return
        if (
          ts.isVariableDeclaration(node) &&
          ts.isIdentifier(node.name) &&
          node.initializer &&
          bindings.has(`${id}:${node.name.text}`) &&
          toolExpression(node.initializer)
        ) {
          edits.push([
            node.initializer.getStart(source),
            node.initializer.end,
            `/*@__PURE__*/(()=>(${node.initializer.getText(source)}))()`,
          ])
          return
        }
        ts.forEachChild(node, annotate)
      }
      annotate(source)
      // Rollup load hooks may already have transpiled TypeScript. All metadata
      // spans refer to the original source, so apply edits before transpilation.
      code = source.text
      for (const [start, end, replacement] of edits.sort((a, b) => b[0] - a[0]))
        code = code.slice(0, start) + replacement + code.slice(end)
      const result = {
        code: `import {Schema as __Schema,SchemaAST as __SchemaAST,Option as __Option,Result as __Result,Effect as __Effect,HashMap as __HashMap} from "effect";import {compiledWire as __wire} from ${literal(path.join(runtime, "internal/compiledWire.js"))};import {registerCompiledTool as __tool} from ${literal(path.join(runtime, "internal/tool/registry.js"))};import {clientCompiled as __toolClient} from ${literal(path.join(runtime, "Tool.js"))};import {registerCompiledAgent as __agent} from ${literal(path.join(runtime, "internal/agent.js"))};import {clientForCompiled as __client} from ${literal(path.join(runtime, "Client.js"))};import {compiledConfigService as __config,compiledConfigRuntime as __configRuntime} from ${literal(path.join(runtime, "internal/compiledConfig.js"))};import {encodeOverrides as __encodeOverrides} from ${literal(path.join(runtime, "Config.js"))};\n${code}`,
        map: null,
      }
      if (/\.[cm]?ts$/.test(id)) {
        const config = ts.findConfigFile(path.dirname(id), ts.sys.fileExists)
        const options = config
          ? ts.parseJsonConfigFileContent(
              ts.readConfigFile(config, ts.sys.readFile).config,
              ts.sys,
              path.dirname(config),
            ).options
          : {}
        result.code = ts.transpileModule(result.code, {
          fileName: id,
          compilerOptions: {
            ...options,
            target: ts.ScriptTarget.ES2022,
            module: ts.ModuleKind.ESNext,
            noEmit: false,
          },
        }).outputText
      }
      transformed.set(id, result)
      return result
    },
  }
}

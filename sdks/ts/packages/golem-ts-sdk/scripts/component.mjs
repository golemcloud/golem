import fs from 'node:fs';
import ts from 'typescript';
import { minify } from 'terser';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
const sdk = manifest.name;
const runtime = path.join(packageRoot, 'dist/runtime');
const componentExports = Object.entries(manifest.exports).flatMap(([subpath, target]) => {
  if (!target || typeof target !== 'object' || typeof target['golem-component'] !== 'string')
    return [];
  if (!target['golem-component'].startsWith('./dist/runtime/'))
    throw new Error(`Invalid ${sdk} component export target: ${target['golem-component']}`);
  return [
    {
      specifier: subpath === '.' ? sdk : `${sdk}/${subpath.slice(2)}`,
      declaration:
        typeof target.types === 'string' ? path.join(packageRoot, target.types) : undefined,
      runtime: path.join(packageRoot, target['golem-component']),
    },
  ];
});
const componentEntries = new Map(
  componentExports.map(({ specifier, runtime }) => [specifier, runtime]),
);
const componentDeclarations = new Map(
  componentExports.flatMap(({ declaration, runtime }) =>
    declaration ? [[path.normalize(declaration), runtime]] : [],
  ),
);

function referencesSdk(node) {
  const specifier =
    ts.isImportDeclaration(node) || ts.isExportDeclaration(node)
      ? node.moduleSpecifier
      : ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument)
        ? node.argument.literal
        : undefined;
  if (
    specifier &&
    ts.isStringLiteralLike(specifier) &&
    (specifier.text === sdk || specifier.text.startsWith(`${sdk}/`))
  )
    return true;
  return ts.forEachChild(node, referencesSdk) ?? false;
}

// Capabilities are inferred from references, not just calls: aliases, destructured
// methods and helper functions can register implementations too. Computed access
// and untyped code retain the full runtime rather than guessing at their effects.
export function discoverCapabilities(parsedConfig) {
  const program = ts.createProgram({
    rootNames: parsedConfig.fileNames,
    options: { ...parsedConfig.options, allowJs: true, maxNodeModuleJsDepth: 20 },
  });
  const checker = program.getTypeChecker();
  const capabilities = { agents: false, tools: false, middleware: false, schemas: false };
  const all = () =>
    Object.assign(capabilities, { agents: true, tools: true, middleware: true, schemas: true });
  const sdkDeclarations = new Set();
  for (const source of program.getSourceFiles()) {
    for (const { specifier } of componentExports) {
      const resolved = ts.resolveModuleName(
        specifier,
        source.fileName,
        parsedConfig.options,
        ts.sys,
      ).resolvedModule;
      if (resolved) sdkDeclarations.add(path.normalize(resolved.resolvedFileName));
    }
  }

  const isSdkSymbol = (symbol) =>
    symbol?.declarations?.some((d) =>
      sdkDeclarations.has(path.normalize(d.getSourceFile().fileName)),
    );
  const isBuilder = (type) => isSdkSymbol(type.getProperty('middleware'));
  const retainBuilder = () => Object.assign(capabilities, { tools: true, middleware: true });
  const packageCapabilities = new Map();
  for (const source of program.getSourceFiles()) {
    const importsSdk = referencesSdk(source);
    if (source.isDeclarationFile) {
      // A third-party declaration can hide registration in its JS implementation.
      if (importsSdk && !sdkDeclarations.has(path.normalize(source.fileName))) all();
      const manifest = ts.findConfigFile(
        path.dirname(source.fileName),
        ts.sys.fileExists,
        'package.json',
      );
      if (manifest && !packageCapabilities.has(manifest)) {
        const metadata = JSON.parse(ts.sys.readFile(manifest));
        packageCapabilities.set(
          manifest,
          metadata.name !== sdk &&
            !!(
              metadata.dependencies?.[sdk] ||
              metadata.peerDependencies?.[sdk] ||
              metadata.optionalDependencies?.[sdk]
            ),
        );
      }
      if (packageCapabilities.get(manifest)) all();
      continue;
    }
    if (source.fileName.startsWith(runtime + path.sep)) continue;
    const visit = (node) => {
      if (ts.isTypeNode(node) || ts.isImportDeclaration(node) || ts.isExportDeclaration(node))
        return;
      if (ts.isCallExpression(node)) {
        const declaration = checker.getResolvedSignature(node)?.declaration;
        // Do not remove registration methods from a builder handed to code whose
        // implementation is unavailable to the compiler (including untyped JS).
        if (
          !declaration ||
          (!declaration.body &&
            !sdkDeclarations.has(path.normalize(declaration.getSourceFile().fileName)))
        ) {
          for (const argument of node.arguments) {
            const type = checker.getTypeAtLocation(argument);
            if (
              isBuilder(type) ||
              type.getCallSignatures().some((s) => isBuilder(s.getReturnType()))
            )
              retainBuilder();
          }
        }
      }
      if (
        (ts.isAsExpression(node) || ts.isTypeAssertionExpression(node)) &&
        checker.getTypeAtLocation(node).flags & ts.TypeFlags.Any &&
        isBuilder(checker.getTypeAtLocation(node.expression))
      )
        retainBuilder();
      if (ts.isElementAccessExpression(node) && !ts.isStringLiteralLike(node.argumentExpression)) {
        const type = checker.getTypeAtLocation(node.expression);
        const properties = ['middleware', 'defineAgent'].map((name) => type.getProperty(name));
        if (
          (importsSdk && type.flags & ts.TypeFlags.Any) ||
          properties.some((p) =>
            p?.declarations?.some((d) =>
              sdkDeclarations.has(path.normalize(d.getSourceFile().fileName)),
            ),
          )
        )
          all();
      }
      if (ts.isIdentifier(node) || ts.isStringLiteralLike(node)) {
        let symbol = checker.getSymbolAtLocation(node);
        if (ts.isBindingElement(node.parent) && ts.isObjectBindingPattern(node.parent.parent)) {
          const name = node.parent.propertyName ?? node.parent.name;
          if (ts.isIdentifier(name) || ts.isStringLiteralLike(name)) {
            symbol = checker.getTypeAtLocation(node.parent.parent).getProperty(name.text) ?? symbol;
          }
        }
        if (symbol?.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
        const declarations = symbol?.declarations ?? [];
        const fromSdk = declarations.some((d) =>
          sdkDeclarations.has(path.normalize(d.getSourceFile().fileName)),
        );
        if (fromSdk) {
          const name = symbol.getName();
          if (name === 'defineAgent' || name === 'defineHttpRouter' || name === 'AgentTypeRegistry')
            capabilities.agents = true;
          if (name === 'durable' || name === 'forSchema') capabilities.schemas = true;
          if (name === 'universalToolMiddleware' || name === 'middleware')
            capabilities.middleware = true;
          if (
            name === 'implement' &&
            declarations.some((d) => d.parent?.name?.text === 'CommandBuilder')
          )
            capabilities.tools = true;
          // Passing the SDK namespace as a value makes every registration reachable.
          if (
            symbol.flags & ts.SymbolFlags.Module &&
            !(ts.isPropertyAccessExpression(node.parent) && node.parent.expression === node)
          )
            all();
        } else if (ts.isPropertyAccessExpression(node.parent) && node.parent.name === node) {
          if (node.text === 'middleware') capabilities.middleware = true;
          if (node.text === 'implement' && !symbol) all();
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return capabilities;
}

export function componentEntry(main, capabilities, nodeHttpRouters) {
  const full = path.join(runtime, 'index.mjs');
  const empty = path.join(runtime, 'emptyGuest.mjs');
  return `
import { guest, saveSnapshot, loadSnapshot } from ${JSON.stringify(capabilities.agents ? full : empty)};
import { tool } from ${JSON.stringify(capabilities.tools ? full : empty)};
import { toolMiddlewareGuest } from ${JSON.stringify(capabilities.middleware ? full : empty)};
${
  nodeHttpRouters === undefined
    ? ''
    : `
import { configureNodeHttpRegistration, closeNodeHttpRegistration } from ${JSON.stringify(path.join(runtime, 'internal/http/nodeHttpRegistration.mjs'))};
export const __golemNodeHttpLifecycle = { close: closeNodeHttpRegistration };
configureNodeHttpRegistration(${JSON.stringify(nodeHttpRouters)});
`
}
export default (async () => {
  await import(${JSON.stringify(main)});
  return { guest, tool, toolMiddlewareGuest, saveSnapshot, loadSnapshot };
})();
`;
}

export function componentPlugin(parsedConfig, main, { nodeHttpRouters = {} } = {}) {
  const capabilities = discoverCapabilities(parsedConfig);
  const automatic =
    !nodeHttpRouters ||
    typeof nodeHttpRouters !== 'object' ||
    Array.isArray(nodeHttpRouters) ||
    Object.keys(nodeHttpRouters).length > 0;
  if (automatic) capabilities.agents = true;
  const entry = '\0golem:component-entry';
  let started = false;
  const validateBuild = (options, watchMode = false) => {
    if (started) throw new Error('The Golem component plugin is one-shot; create a fresh plugin');
    if (watchMode || options.watch)
      throw new Error('The Golem component plugin does not support watch mode');
    if (options.cache) throw new Error('The Golem component plugin does not support Rollup cache');
  };
  return {
    name: 'golem-component',
    options(options) {
      validateBuild(options);
      const external = options.external;
      return {
        ...options,
        external(id, importer, resolved) {
          // HTTP imports must reach resolveId before the caller's node:* predicate.
          if (id === 'http' || id === 'node:http') return false;
          if (typeof external === 'function') return external(id, importer, resolved);
          return (Array.isArray(external) ? external : external ? [external] : []).some((entry) =>
            typeof entry === 'string' ? entry === id : entry.test(id),
          );
        },
      };
    },
    buildStart(options) {
      validateBuild(options, this.meta.watchMode);
      started = true;
    },
    resolveId(id, importer) {
      if (id === 'virtual:agent-main') return entry;
      if (id === 'http' || id === 'node:http') {
        const facade = path.join(runtime, 'nodeHttp.mjs');
        // Only the facade may bypass itself to the untouched runtime builtin.
        return importer === facade ? { id: 'node:http', external: true } : facade;
      }
      const componentEntry = componentEntries.get(id);
      if (componentEntry) return componentEntry;
      if (id === sdk || id.startsWith(`${sdk}/`))
        this.error(`Package import ${id} is not available to component builds`);
      if (importer && !importer.startsWith('\0') && !importer.startsWith(runtime + path.sep)) {
        const resolved = ts.resolveModuleName(
          id,
          importer,
          parsedConfig.options,
          ts.sys,
        ).resolvedModule;
        const componentEntry = componentDeclarations.get(
          path.normalize(resolved?.resolvedFileName ?? ''),
        );
        if (componentEntry) return componentEntry;
      }
    },
    load(id) {
      if (id === entry)
        return componentEntry(main, capabilities, automatic ? nodeHttpRouters : undefined);
    },
    async renderChunk(code, _chunk, options) {
      if (automatic) {
        if (options.format !== 'es' || !options.inlineDynamicImports)
          this.error(
            'Node HTTP auto-registration requires a single ES module with inlineDynamicImports',
          );
        const statements = this.parse(code).body;
        const exports = statements.filter((node) => node.type === 'ExportNamedDeclaration');
        const specifiers = exports.flatMap((node) => node.specifiers);
        const result = specifiers.find((node) => node.exported.name === 'default')?.local.name;
        const lifecycle = specifiers.find(
          (node) => node.exported.name === '__golemNodeHttpLifecycle',
        )?.local.name;
        const declaration = statements.find(
          (node) =>
            node.type === 'VariableDeclaration' &&
            node.declarations.some((item) => item.id.name === lifecycle),
        );
        if (
          !result ||
          !lifecycle ||
          specifiers.length !== 2 ||
          !declaration ||
          declaration.declarations.length !== 1 ||
          statements.some((node) => node.type === 'ExportDefaultDeclaration')
        )
          this.error('Unexpected Node HTTP component initialization export shape');
        const imports = statements
          .filter((node) => node.type === 'ImportDeclaration')
          .map((node) => code.slice(node.start, node.end))
          .join('\n');
        const body = statements
          .filter(
            (node) => node.type !== 'ImportDeclaration' && node.type !== 'ExportNamedDeclaration',
          )
          .map((node) =>
            node === declaration
              ? `var${code.slice(node.start + node.kind.length, node.end)}`
              : code.slice(node.start, node.end),
          )
          .join('\n');
        // Inline imports only await a namespace promise. This factory brackets actual evaluation,
        // including application/dependency top-level await and synchronous module failures.
        code = `${imports}\nexport default (async () => {
try { ${body}\nreturn await ${result}; }
finally { ${lifecycle}?.close(); }
})();`;
      }
      const result = await minify(code, {
        module: options.format === 'es',
        keep_fnames: true,
        keep_classnames: true,
      });
      return { code: result.code, map: null };
    },
  };
}

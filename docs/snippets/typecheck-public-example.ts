import { existsSync } from "node:fs"
import { resolve } from "node:path"
import ts from "typescript"

// Resolve each example beside its real SDK so Effect v4 and the plain SDK's
// schema libraries come from their respective locked dependency installations.
export function typecheckPublicExample(code: string): string[] {
  const effect = code.includes('from "effect"')
  const sdk = resolve(
    import.meta.dir,
    effect ? "../../sdks/effect" : "../../sdks/ts/packages/golem-ts-sdk"
  )
  const declaration = resolve(sdk, effect ? "dist/effect-golem.d.ts" : "dist/index.d.mts")
  if (!existsSync(declaration)) {
    throw new Error(`Build the in-tree SDK before checking public snippets: missing ${declaration}`)
  }
  const filename = resolve(sdk, "public-doc-example.mts")
  const options: ts.CompilerOptions = {
    strict: true,
    noEmit: true,
    skipLibCheck: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    lib: ["lib.es2022.d.ts", "lib.dom.d.ts", "lib.dom.iterable.d.ts"],
    types: [],
    paths: {
      [effect ? "@golemcloud/effect-golem" : "@golemcloud/golem-ts-sdk"]: [declaration],
    },
  }
  const host = ts.createCompilerHost(options)
  const readFile = host.readFile.bind(host)
  const fileExists = host.fileExists.bind(host)
  host.readFile = path => (path === filename ? code : readFile(path))
  host.fileExists = path => path === filename || fileExists(path)
  const program = ts.createProgram([filename], options, host)
  return ts
    .getPreEmitDiagnostics(program)
    .filter(diagnostic => diagnostic.category === ts.DiagnosticCategory.Error)
    .map(diagnostic => ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"))
}

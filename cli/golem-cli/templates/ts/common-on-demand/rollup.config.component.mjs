import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import ts from "typescript";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

// Rollup config for a TypeScript agent component.
//
// The component's tsconfig.json is the single source of truth: it decides which
// files are compiled (`include`/`files`) and how module path aliases resolve
// (`compilerOptions.paths`). This config reads it once and defers to
// @rollup/plugin-typescript rather than restating any of it here, so the build
// and the type checker always agree on the same file set and resolution rules.
//
// The SDK derives agent metadata at runtime from the schemas, so the
// virtual entry only imports the user's main module for its side-effecting
// `defineAgent(...).implement(...)` registrations. The SDK package, its supported
// subpaths, and the `golem:*` host packages are externalized (provided by the
// selected prebuilt wrapper); user code and the schema library are bundled into
// main.js and injected into that wasm.

// Read tsconfig.json through the TypeScript compiler API — the same path
// @rollup/plugin-typescript takes — so comments and `extends` are honored, and a
// missing or invalid tsconfig fails the build with a clear error instead of being
// ignored and producing a confusing failure further down the pipeline.
function loadComponentTsConfig(componentDir) {
    const tsconfigPath = path.join(componentDir, "tsconfig.json");
    if (!fs.existsSync(tsconfigPath)) {
        throw new Error(`tsconfig.json not found at ${tsconfigPath}`);
    }

    const { config, error } = ts.readConfigFile(tsconfigPath, ts.sys.readFile);
    if (error) {
        throw new Error(
            `Failed to read ${tsconfigPath}: ${ts.flattenDiagnosticMessageText(error.messageText, "\n")}`,
        );
    }

    const parsed = ts.parseJsonConfigFileContent(config, ts.sys, componentDir);
    const errors = parsed.errors.filter((d) => d.category === ts.DiagnosticCategory.Error);
    if (errors.length > 0) {
        const message = errors
            .map((d) => ts.flattenDiagnosticMessageText(d.messageText, "\n"))
            .join("\n");
        throw new Error(`Invalid ${tsconfigPath}:\n${message}`);
    }

    return parsed;
}

const sdkPackage = "@golemcloud/golem-ts-sdk";
const middlewareSdkPackage = `${sdkPackage}/middleware`;
const componentRoles = {
    agent: {
        template: "ts",
        sdkImport: sdkPackage,
    },
    "tool-middleware": {
        template: "ts-tool-middleware",
        sdkImport: middlewareSdkPackage,
    },
    "agent-tool-middleware": {
        template: "ts-agent-tool-middleware",
        sdkImport: sdkPackage,
    },
};

function visit(node, callback) {
    callback(node);
    ts.forEachChild(node, (child) => visit(child, callback));
}

function importedModule(node) {
    if (
        (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
        node.moduleSpecifier &&
        ts.isStringLiteralLike(node.moduleSpecifier)
    ) {
        return node.moduleSpecifier.text;
    }
    if (
        ts.isImportEqualsDeclaration(node) &&
        ts.isExternalModuleReference(node.moduleReference) &&
        node.moduleReference.expression &&
        ts.isStringLiteralLike(node.moduleReference.expression)
    ) {
        return node.moduleReference.expression.text;
    }
    if (
        ts.isCallExpression(node) &&
        node.expression.kind === ts.SyntaxKind.ImportKeyword &&
        node.arguments.length === 1 &&
        ts.isStringLiteralLike(node.arguments[0])
    ) {
        return node.arguments[0].text;
    }
    if (
        ts.isImportTypeNode(node) &&
        ts.isLiteralTypeNode(node.argument) &&
        ts.isStringLiteralLike(node.argument.literal)
    ) {
        return node.argument.literal.text;
    }
}

function sourceLocation(sourceFile, node) {
    const { line, character } = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
    return `${path.relative(process.cwd(), sourceFile.fileName)}:${line + 1}:${character + 1}`;
}

function normalizedFileName(fileName) {
    const realPath = ts.sys.realpath ? ts.sys.realpath(fileName) : fileName;
    const normalized = path.normalize(realPath);
    return ts.sys.useCaseSensitiveFileNames ? normalized : normalized.toLowerCase();
}

function declarationName(declaration) {
    const name = declaration?.name;
    return name && (ts.isIdentifier(name) || ts.isStringLiteralLike(name)) ? name.text : undefined;
}

function isSdkModule(moduleName) {
    return moduleName === sdkPackage || moduleName.startsWith(`${sdkPackage}/`);
}

function resolveModule(moduleName, sourceFile, compilerOptions) {
    return ts.resolveModuleName(
        moduleName,
        sourceFile.fileName,
        compilerOptions,
        ts.sys,
    ).resolvedModule;
}

function validateComponentRole(parsedTsConfig, roleName) {
    const role = componentRoles[roleName];
    if (!role) {
        throw new Error(
            `Unknown GOLEM_COMPONENT_ROLE ${JSON.stringify(roleName)}. Expected one of: ${Object.keys(componentRoles).join(", ")}`,
        );
    }

    const program = ts.createProgram({
        rootNames: parsedTsConfig.fileNames,
        options: parsedTsConfig.options,
    });
    const checker = program.getTypeChecker();
    const rootFileNames = new Set(parsedTsConfig.fileNames.map(normalizedFileName));
    const userSources = program
        .getSourceFiles()
        .filter((sourceFile) => !sourceFile.isDeclarationFile)
        .filter((sourceFile) => rootFileNames.has(normalizedFileName(sourceFile.fileName)));
    const violations = [];
    const sdkEntriesBySource = new Map();
    const sdkDeclarationFiles = new Set();

    for (const sourceFile of userSources) {
        const sdkEntries = new Map();
        for (const sdkEntry of [sdkPackage, middlewareSdkPackage]) {
            const resolved = resolveModule(sdkEntry, sourceFile, parsedTsConfig.options);
            if (!resolved) continue;
            const resolvedFileName = normalizedFileName(resolved.resolvedFileName);
            sdkEntries.set(resolvedFileName, sdkEntry);
            if (sdkEntry === sdkPackage) sdkDeclarationFiles.add(resolvedFileName);
        }
        sdkEntriesBySource.set(sourceFile, sdkEntries);
    }

    for (const sourceFile of userSources) {
        visit(sourceFile, (node) => {
            const moduleName = importedModule(node);
            if (!moduleName) return;

            const directSdkModule = isSdkModule(moduleName);
            const resolved = resolveModule(moduleName, sourceFile, parsedTsConfig.options);
            if (!resolved) {
                if (directSdkModule) {
                    violations.push(
                        `${sourceLocation(sourceFile, node)} cannot resolve ${JSON.stringify(moduleName)}, which is required to validate component template ${JSON.stringify(role.template)}.`,
                    );
                }
                return;
            }

            const sdkModule = directSdkModule
                ? moduleName
                : sdkEntriesBySource
                      .get(sourceFile)
                      ?.get(normalizedFileName(resolved.resolvedFileName));
            if (!sdkModule && resolved.packageId?.name === sdkPackage) {
                violations.push(
                    `${sourceLocation(sourceFile, node)} imports ${JSON.stringify(moduleName)}, which resolves to an unsupported ${JSON.stringify(sdkPackage)} entry point; component template ${JSON.stringify(role.template)} requires ${JSON.stringify(role.sdkImport)}.`,
                );
                return;
            }
            if (!sdkModule || sdkModule === role.sdkImport) return;

            const resolvedThroughAlias = moduleName === sdkModule
                ? ""
                : `, which resolves to ${JSON.stringify(sdkModule)}`;
            violations.push(
                `${sourceLocation(sourceFile, node)} imports ${JSON.stringify(moduleName)}${resolvedThroughAlias}, but component template ${JSON.stringify(role.template)} requires ${JSON.stringify(role.sdkImport)}.`,
            );
        });
    }

    if (roleName === "agent") {
        for (const sourceFile of userSources) {
            visit(sourceFile, (node) => {
                if (!ts.isCallExpression(node)) return;
                const declaration = checker.getResolvedSignature(node)?.declaration;
                if (
                    !declaration ||
                    !sdkDeclarationFiles.has(normalizedFileName(declaration.getSourceFile().fileName)) ||
                    !["middleware", "universalToolMiddleware"].includes(declarationName(declaration))
                ) {
                    return;
                }
                violations.push(
                    `${sourceLocation(sourceFile, node)} defines tool middleware, but component template "ts" does not export tool middleware. Use "ts-tool-middleware" for middleware only or "ts-agent-tool-middleware" for agents, tools, and middleware together.`,
                );
            });
        }
    }

    if (violations.length > 0) {
        throw new Error(`Invalid TypeScript component role:\n${violations.join("\n")}`);
    }
}

function componentRollupConfig() {
    const componentName = process.env.GOLEM_COMPONENT_NAME;
    if (!componentName) {
        throw new Error("GOLEM_COMPONENT_NAME is not set");
    }
    const golemTemp = process.env.GOLEM_TEMP;
    if (!golemTemp) {
        throw new Error("GOLEM_TEMP is not set");
    }
    const componentRole = process.env.GOLEM_COMPONENT_ROLE;
    if (!componentRole) {
        throw new Error("GOLEM_COMPONENT_ROLE is not set");
    }

    const componentDir = process.cwd();
    const parsedTsConfig = loadComponentTsConfig(componentDir);

    // Compile exactly the files the tsconfig resolves. `parsed.fileNames` is
    // TypeScript's own expansion of `include`/`files`/`exclude`, which excludes
    // node_modules by default. Scoping the plugin to this set keeps dependencies
    // out of TypeScript compilation: a package that ships `.ts` sources next to
    // its compiled `.js` would otherwise be dragged in through TypeScript's
    // `.js`->`.ts` source redirect, and rollup would fail parsing that `.ts` as
    // plain JavaScript. Path aliases (`compilerOptions.paths`) are resolved by the
    // plugin itself (`ts.resolveModuleName`), so no separate alias plugin is
    // needed. Fall back to the conventional `src/` glob only when the tsconfig
    // resolves no files.
    const include = parsedTsConfig.fileNames.length > 0
        ? parsedTsConfig.fileNames
        : ["./src/**/*.ts"];

    const externalSdkModules = new Set([sdkPackage, `${sdkPackage}/middleware`]);
    const externalPackages = (id) =>
        externalSdkModules.has(id) || id.startsWith("golem:");

    const virtualAgentMainId = "virtual:agent-main";
    const resolvedVirtualAgentMainId = "\0virtual:agent-main";
    const virtualAgentMainPlugin = () => ({
        name: "agent-main",
        resolveId(id) {
            if (id === virtualAgentMainId) {
                return resolvedVirtualAgentMainId;
            }
        },
        load(id) {
            if (id === resolvedVirtualAgentMainId) {
                // Async wrapper keeps rollup from reordering the side-effecting import.
                return `export default (async () => { return await import("./src/main"); })();`;
            }
        },
    });

    const plugins = [
        {
            name: "component-role",
            buildStart() {
                validateComponentRole(parsedTsConfig, componentRole);
            },
        },
        virtualAgentMainPlugin(),
        nodeResolve({ extensions: [".mjs", ".js", ".node", ".ts"] }),
        commonjs(),
        json(),
        typescript({
            noEmitOnError: true,
            include,
        }),
    ];

    return {
        input: virtualAgentMainId,
        output: {
            file: path.join(golemTemp, "ts-dist", componentName, "main.js"),
            format: "esm",
            inlineDynamicImports: true,
            sourcemap: false,
        },
        external: externalPackages,
        plugins,
    };
}

export default componentRollupConfig();

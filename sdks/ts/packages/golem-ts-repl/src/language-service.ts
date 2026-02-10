// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { Config, ProcessArgs } from './config';

import tsm, { ts } from 'ts-morph';
import pc from 'picocolors';

export type SnippetTypeCheckResult =
  | {
      ok: true;
    }
  | {
      ok: false;
      formattedErrors: string;
    };

export type SnippetQuickInfo = {
  formattedInfo: string;
};

export type SnippetTypeInfo = {
  formattedType: string;
  isPromise: boolean;
};

export type SnippetCompletion = {
  entries: string[];
  memberCompletion: boolean;
  replaceStart: number;
  replaceEnd: number;
};

const SNIPPET_FILE_NAME = '__snippet__.ts';

export class LanguageService {
  private readonly snippetImports;
  private project: tsm.Project;
  private snippetHistory: string;
  private rawSnippet: string;
  private snippetEndPos: number;
  private snippetStartPos: number;

  constructor(config: Config, processArgs: ProcessArgs) {
    this.snippetImports = processArgs.disableAutoImports
      ? ''
      : Object.entries(config.agents)
          .map(([agentTypeName, agentConfig]) => {
            return [
              `import { ${agentTypeName} } from '${agentConfig.clientPackageName}';`,
              `import * as ${agentConfig.clientPackageImportedName} from '${agentConfig.clientPackageName}';`,
            ].join('\n');
          })
          .join('\n') + '\n';

    this.project = new tsm.Project({
      tsConfigFilePath: 'tsconfig.json',
      skipAddingFilesFromTsConfig: false,
    });

    this.snippetHistory = this.snippetImports;
    this.rawSnippet = '';
    this.snippetEndPos = 0;
    this.snippetStartPos = 0;
  }

  private updateProjectSnippet() {
    const fullSnippet = this.snippetHistory + this.rawSnippet;
    this.snippetStartPos = fullSnippet.length - this.rawSnippet.length;
    this.snippetEndPos = lastNonWhitespaceIndex(fullSnippet);
    if (this.snippetEndPos < this.snippetHistory.length) {
      this.snippetEndPos = -1;
    }
    this.project.createSourceFile(SNIPPET_FILE_NAME, fullSnippet, { overwrite: true });
  }

  addSnippetToHistory(snippet: string) {
    if (!this.snippetHistory.endsWith('\n')) {
      snippet = snippet + '\n';
    }
    this.snippetHistory = this.snippetHistory + snippet;
    this.updateProjectSnippet();
  }

  setSnippet(snippet: string) {
    this.rawSnippet = snippet;
    this.updateProjectSnippet();
  }

  private getSnippet(): tsm.SourceFile | undefined {
    return this.project.getSourceFile(SNIPPET_FILE_NAME);
  }

  typeCheckSnippet(): SnippetTypeCheckResult {
    const snippet = this.getSnippet();
    if (!snippet) {
      return { ok: true };
    }

    const diagnostics = snippet.getPreEmitDiagnostics();
    const errors = diagnostics.filter((d) => d.getCategory() === ts.DiagnosticCategory.Error);

    if (errors.length === 0) {
      return { ok: true };
    } else {
      return {
        ok: false,
        formattedErrors: this.project.formatDiagnosticsWithColorAndContext(errors),
      };
    }
  }

  getSnippetQuickInfo(): SnippetQuickInfo | undefined {
    const snippet = this.getSnippet();
    if (!snippet) return;

    const languageService = this.project.getLanguageService();
    const tsLs = languageService.compilerObject;

    const info = tsLs.getQuickInfoAtPosition(snippet.getFilePath(), this.snippetEndPos);

    if (!info) return;

    let formattedInfo = '';

    if (info.displayParts?.length) {
      formattedInfo += formatDisplayParts(info.displayParts);
    }

    return { formattedInfo };
  }

  getSnippetTypeInfo(): SnippetTypeInfo | undefined {
    if (this.snippetEndPos === -1) return;

    const snippet = this.getSnippet();
    if (!snippet) return;

    const node = snippet.getDescendantAtPos(this.snippetEndPos);
    if (!node) return;

    const fullExpressionNode = getFullExpression(node);
    if (fullExpressionNode.getKind() === ts.SyntaxKind.SourceFile) {
      return;
    }

    let nodeType;
    try {
      nodeType = fullExpressionNode.getType();
    } catch (e) {
      console.error();
      console.error('If you see this, please report it!');
      console.error(fullExpressionNode);
      console.error(e);
      console.error();
    }
    if (!nodeType) return;

    const typeIsPromise = isPromise(nodeType);
    const typeAsLiteralType = typeIsPromise ? undefined : asLiteralType(nodeType);

    const typeText = typeAsLiteralType
      ? typeAsLiteralType
      : this.project.getTypeChecker().getTypeText(nodeType, fullExpressionNode);

    const formattedType = formatTypeText(typeText);

    return {
      formattedType,
      isPromise: typeIsPromise,
    };
  }

  getSnippetCompletions(): SnippetCompletion | undefined {
    const snippet = this.getSnippet();
    if (!snippet) return;

    let pos = this.snippetEndPos;

    const triggerCharacter = matchTriggerCharacter(snippet.getText(), pos);
    const triggerKind = triggerCharacter
      ? ts.CompletionTriggerKind.TriggerCharacter
      : ts.CompletionTriggerKind.Invoked;

    const rawStart = this.snippetStartPos;
    const rawEnd = rawStart + this.rawSnippet.length;

    const placeholderResult =
      triggerCharacter === '.'
        ? undefined
        : this.getSnippetPlaceholderCompletions(snippet, pos, rawStart);

    const tsLs = this.project.getLanguageService().compilerObject;

    const completions = tsLs.getCompletionsAtPosition(snippet.getFilePath(), pos + 1, {
      triggerKind,
      triggerCharacter,
      includeCompletionsForModuleExports: false,
      includeCompletionsForImportStatements: false,
      includeCompletionsWithInsertText: false,
      includeCompletionsWithSnippetText: false,
    });

    if (!completions) {
      if (placeholderResult?.entries.length) {
        return {
          entries: placeholderResult.entries,
          memberCompletion: false,
          replaceStart: placeholderResult.replaceStart,
          replaceEnd: placeholderResult.replaceEnd,
        };
      }
      return;
    }

    if (triggerCharacter === '.') {
      const memberLikeKinds = new Set<ts.ScriptElementKind>([
        ts.ScriptElementKind.memberVariableElement,
        ts.ScriptElementKind.memberFunctionElement,
        ts.ScriptElementKind.memberGetAccessorElement,
        ts.ScriptElementKind.memberSetAccessorElement,
        ts.ScriptElementKind.memberAccessorVariableElement,
        ts.ScriptElementKind.variableElement,
        ts.ScriptElementKind.letElement,
        ts.ScriptElementKind.constElement,
        ts.ScriptElementKind.functionElement,
        ts.ScriptElementKind.classElement,
        ts.ScriptElementKind.interfaceElement,
        ts.ScriptElementKind.typeElement,
        ts.ScriptElementKind.enumElement,
        ts.ScriptElementKind.enumMemberElement,
        ts.ScriptElementKind.moduleElement,
        ts.ScriptElementKind.alias,
      ]);

      const filteredEntries = completions.entries
        .filter((entry) => memberLikeKinds.has(entry.kind))
        .map((entry) => entry.name);

      return {
        entries: filteredEntries.length
          ? filteredEntries
          : completions.entries.map((entry) => entry.name),
        memberCompletion: true,
        replaceStart: Math.max(0, pos + 1 - rawStart),
        replaceEnd: Math.max(0, rawEnd - rawStart),
      };
    } else {
      const node = snippet.getDescendantAtPos(this.snippetEndPos);
      if (!node) {
        if (placeholderResult?.entries.length) {
          return {
            entries: placeholderResult.entries,
            memberCompletion: false,
            replaceStart: placeholderResult.replaceStart,
            replaceEnd: placeholderResult.replaceEnd,
          };
        } else {
          return {
            entries: completions.entries.map((entry) => entry.name),
            memberCompletion: false,
            replaceStart: 0,
            replaceEnd: this.rawSnippet.length,
          };
        }
      }

      const parent = node.getParent();
      const memberCompletion = parent
        ? parent.getKind() === ts.SyntaxKind.PropertyAccessExpression
        : false;

      const nodeText = node.getText();
      const nodeStart = node.getStart();
      const nodeEnd = node.getEnd();

      const completionEntries = completions.entries
        .filter((entry) => entry.name.startsWith(nodeText))
        .map((entry) => entry.name);

      if (placeholderResult?.entries.length) {
        const merged = mergeCompletionEntries(placeholderResult.entries, completionEntries);
        return {
          entries: merged,
          memberCompletion: false,
          replaceStart: placeholderResult.replaceStart,
          replaceEnd: placeholderResult.replaceEnd,
        };
      }

      return {
        entries: completionEntries,
        memberCompletion,
        replaceStart: Math.max(0, nodeStart - rawStart),
        replaceEnd: Math.max(0, nodeEnd - rawStart),
      };
    }
  }

  private getSnippetPlaceholderCompletions(
    snippet: tsm.SourceFile,
    pos: number,
    rawStart: number,
  ): { entries: string[]; replaceStart: number; replaceEnd: number } | undefined {
    if (pos < 0) return;

    const checker = this.project.getTypeChecker();
    const context = getCallContextForPlaceholders(snippet, pos, rawStart);
    if (!context) return;

    const { signature, argIndex, argNode, replaceRange, expressionNode } = context;
    if (!signature) return;
    if (signature.getParameters().length === 0) {
      const snippetText = snippet.getText();
      if (snippetText[pos + 1] === ')') return;
      return {
        entries: [')'],
        replaceStart: replaceRange.replaceStart,
        replaceEnd: replaceRange.replaceEnd,
      };
    }
    const paramType = getSignatureParameterType(signature, argIndex, checker, expressionNode);
    if (!paramType) return;

    const placeholders = getArgumentTypePlaceholders(paramType, checker, argNode);
    if (!placeholders.length) return;

    let filtered = placeholders;
    const prefix = getArgumentPrefixText(snippet, argNode, pos);
    if (prefix) {
      filtered = placeholders.filter((entry) => entry.startsWith(prefix));
    }

    if (!filtered.length) return;

    return {
      entries: filtered.slice(0, MAX_PLACEHOLDER_ENTRIES),
      replaceStart: replaceRange.replaceStart,
      replaceEnd: replaceRange.replaceEnd,
    };
  }
}

function matchTriggerCharacter(
  text: string,
  pos: number,
): ts.CompletionsTriggerCharacter | undefined {
  const i = Math.max(0, pos);
  const ch = text[i];

  if (
    ch === '.' ||
    ch === '"' ||
    ch === "'" ||
    ch === '`' ||
    ch === '/' ||
    ch === '@' ||
    ch === '<' ||
    ch === '#'
  ) {
    return ch as ts.CompletionsTriggerCharacter;
  }
  return undefined;
}

type CallArgumentContext = {
  callExpression: tsm.CallExpression | tsm.NewExpression;
  argIndex: number;
  argNode: tsm.Expression | undefined;
};

type PlaceholderCallContext = {
  expressionNode: tsm.Expression;
  signature: tsm.Signature;
  argIndex: number;
  argNode: tsm.Expression | undefined;
  replaceRange: { replaceStart: number; replaceEnd: number };
};

function getCallContextForPlaceholders(
  snippet: tsm.SourceFile,
  pos: number,
  rawStart: number,
): PlaceholderCallContext | undefined {
  const callContext = getCallArgumentContext(snippet, pos);
  if (callContext) {
    const signature = getResolvedSignature(callContext.callExpression);
    if (!signature) return;
    const replaceRange = getArgumentReplaceRange(
      callContext.callExpression,
      callContext.argIndex,
      pos,
      rawStart,
    );
    return {
      expressionNode: callContext.callExpression.getExpression(),
      signature,
      argIndex: callContext.argIndex,
      argNode: callContext.argNode,
      replaceRange,
    };
  }

  const fallbackContext = getFallbackCallContext(snippet, pos, rawStart);
  if (!fallbackContext) return;
  return fallbackContext;
}

function getCallArgumentContext(
  snippet: tsm.SourceFile,
  pos: number,
): CallArgumentContext | undefined {
  const snippetText = snippet.getText();
  const candidates = [
    Math.min(pos, snippet.getEnd()),
    Math.min(pos + 1, snippet.getEnd()),
    Math.max(0, pos - 1),
  ];

  let callExpression: tsm.CallExpression | tsm.NewExpression | undefined;
  for (const candidate of candidates) {
    const node = snippet.getDescendantAtPos(candidate);
    if (!node) continue;

    callExpression =
      node.getFirstAncestorByKind(tsm.SyntaxKind.CallExpression) ??
      node.getFirstAncestorByKind(tsm.SyntaxKind.NewExpression);

    if (callExpression) break;
  }

  if (!callExpression) {
    const allCalls = [
      ...snippet.getDescendantsOfKind(tsm.SyntaxKind.CallExpression),
      ...snippet.getDescendantsOfKind(tsm.SyntaxKind.NewExpression),
    ];

    callExpression = allCalls
      .filter((expr) => {
        const openParen = expr.getFirstChildByKind(tsm.SyntaxKind.OpenParenToken);
        if (!openParen) return false;
        const closeParen = expr.getFirstChildByKind(tsm.SyntaxKind.CloseParenToken);
        const closePos = closeParen ? closeParen.getStart() : snippet.getEnd();
        return pos >= openParen.getStart() && pos <= closePos;
      })
      .sort((a, b) => b.getStart() - a.getStart())[0];
  }

  if (!callExpression) return;

  const openParen = callExpression.getFirstChildByKind(tsm.SyntaxKind.OpenParenToken);
  const closeParen = callExpression.getFirstChildByKind(tsm.SyntaxKind.CloseParenToken);
  if (!openParen) return;

  const closePos = closeParen ? closeParen.getStart() : snippet.getEnd();
  if (pos < openParen.getStart() || pos > closePos) return;

  const args = callExpression.getArguments();
  let argIndex = args.length;
  for (let i = 0; i < args.length; i++) {
    if (pos <= args[i].getEnd()) {
      argIndex = i;
      break;
    }
  }

  if (snippetText[pos] === ',' && argIndex < args.length) {
    argIndex = Math.min(argIndex + 1, args.length);
  }

  const argNode = argIndex < args.length ? (args[argIndex] as tsm.Expression) : undefined;

  return {
    callExpression,
    argIndex,
    argNode,
  };
}

function getFallbackCallContext(
  snippet: tsm.SourceFile,
  pos: number,
  rawStart: number,
): PlaceholderCallContext | undefined {
  const snippetText = snippet.getText();
  const openParenPos = findOpenParenForCall(snippetText, pos);
  if (openParenPos === undefined || openParenPos < rawStart) return;

  const nodeBefore = snippet.getDescendantAtPos(Math.max(0, openParenPos - 1));
  if (!nodeBefore) return;

  const expressionNode = getCallTargetExpression(nodeBefore, openParenPos);
  if (!expressionNode) return;

  const signature = getSignatureFromType(expressionNode.getType(), false);
  if (!signature) return;

  const argInfo = getFallbackArgumentInfo(snippetText, openParenPos, pos);
  if (!argInfo) return;

  const replaceStart = Math.max(0, argInfo.replaceStartAbs - rawStart);
  const replaceEnd = Math.max(0, pos + 1 - rawStart);

  return {
    expressionNode,
    signature,
    argIndex: argInfo.argIndex,
    argNode: undefined,
    replaceRange: { replaceStart, replaceEnd },
  };
}

function getCallTargetExpression(node: tsm.Node, pos: number): tsm.Expression | undefined {
  let current: tsm.Node | undefined = node;
  while (current) {
    if (
      tsm.Node.isPropertyAccessExpression(current) ||
      tsm.Node.isElementAccessExpression(current)
    ) {
      if (current.getEnd() >= pos) {
        return current as tsm.Expression;
      }
    }
    if (
      tsm.Node.isIdentifier(current) ||
      tsm.Node.isThisExpression(current) ||
      tsm.Node.isSuperExpression(current)
    ) {
      return current as tsm.Expression;
    }
    current = current.getParent();
  }
  return undefined;
}

function findOpenParenForCall(text: string, pos: number): number | undefined {
  const scanner = ts.createScanner(
    ts.ScriptTarget.Latest,
    false,
    ts.LanguageVariant.Standard,
    text,
  );
  const stack: number[] = [];

  for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
    const tokenStart = scanner.getTokenStart();
    if (tokenStart > pos) break;
    if (token === ts.SyntaxKind.OpenParenToken) {
      stack.push(tokenStart);
    } else if (token === ts.SyntaxKind.CloseParenToken) {
      stack.pop();
    }
  }

  return stack.length ? stack[stack.length - 1] : undefined;
}

function getFallbackArgumentInfo(
  text: string,
  openParenPos: number,
  pos: number,
): { argIndex: number; replaceStartAbs: number } | undefined {
  const scanner = ts.createScanner(
    ts.ScriptTarget.Latest,
    false,
    ts.LanguageVariant.Standard,
    text,
  );

  let parenDepth = 0;
  let bracketDepth = 0;
  let braceDepth = 0;
  let commaCount = 0;
  let lastComma = -1;

  for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
    const tokenStart = scanner.getTokenStart();
    if (tokenStart < openParenPos + 1) continue;
    if (tokenStart > pos) break;

    if (token === ts.SyntaxKind.OpenParenToken) {
      if (tokenStart !== openParenPos) {
        parenDepth++;
      }
      continue;
    }
    if (token === ts.SyntaxKind.CloseParenToken) {
      if (parenDepth === 0) {
        return undefined;
      }
      parenDepth--;
      continue;
    }
    if (token === ts.SyntaxKind.OpenBracketToken) {
      bracketDepth++;
      continue;
    }
    if (token === ts.SyntaxKind.CloseBracketToken) {
      bracketDepth = Math.max(0, bracketDepth - 1);
      continue;
    }
    if (token === ts.SyntaxKind.OpenBraceToken) {
      braceDepth++;
      continue;
    }
    if (token === ts.SyntaxKind.CloseBraceToken) {
      braceDepth = Math.max(0, braceDepth - 1);
      continue;
    }
    if (token === ts.SyntaxKind.CommaToken) {
      if (parenDepth === 0 && bracketDepth === 0 && braceDepth === 0) {
        commaCount++;
        lastComma = tokenStart;
      }
    }
  }

  const replaceStartAbs = lastComma >= 0 ? lastComma + 1 : openParenPos + 1;
  return { argIndex: commaCount, replaceStartAbs };
}

function getResolvedSignature(
  callExpression: tsm.CallExpression | tsm.NewExpression,
): tsm.Signature | undefined {
  const directSignature = (callExpression as any).getSignature?.();
  if (directSignature) return directSignature;

  const isNew = callExpression.getKind() === tsm.SyntaxKind.NewExpression;
  const expressionType = callExpression.getExpression().getType();
  return getSignatureFromType(expressionType, isNew);
}

function findSignatureFromType(type: tsm.Type, isNew: boolean): tsm.Signature | undefined {
  const signatures = isNew ? type.getConstructSignatures() : type.getCallSignatures();
  if (signatures.length) return signatures[0];
  return undefined;
}

function getSignatureFromType(type: tsm.Type, isNew: boolean): tsm.Signature | undefined {
  const signature = findSignatureFromType(type, isNew);
  if (signature) return signature;

  const apparent = type.getApparentType();
  const apparentSignature = findSignatureFromType(apparent, isNew);
  if (apparentSignature) return apparentSignature;

  if (type.isIntersection()) {
    for (const intersectionType of type.getIntersectionTypes()) {
      const intersectionSignature = findSignatureFromType(intersectionType, isNew);
      if (intersectionSignature) return intersectionSignature;
    }
  }

  if (apparent.isIntersection()) {
    for (const intersectionType of apparent.getIntersectionTypes()) {
      const intersectionSignature = findSignatureFromType(intersectionType, isNew);
      if (intersectionSignature) return intersectionSignature;
    }
  }

  if (type.isUnion()) {
    for (const unionType of type.getUnionTypes()) {
      const unionSignature = findSignatureFromType(unionType, isNew);
      if (unionSignature) return unionSignature;
    }
  }

  if (apparent.isUnion()) {
    for (const unionType of apparent.getUnionTypes()) {
      const unionSignature = findSignatureFromType(unionType, isNew);
      if (unionSignature) return unionSignature;
    }
  }

  return undefined;
}

function getSignatureParameterType(
  signature: tsm.Signature,
  argIndex: number,
  checker: tsm.TypeChecker,
  location: tsm.Node,
): tsm.Type | undefined {
  const params = signature.getParameters();
  if (!params.length) return;

  let paramSymbol = params[Math.min(argIndex, params.length - 1)];
  const contextualType = checker.getTypeOfSymbolAtLocation(paramSymbol, location);
  const decl = paramSymbol.getDeclarations()[0];
  const typeAtDecl = decl ? checker.getTypeAtLocation(decl) : undefined;
  const resolvedType = contextualType ?? typeAtDecl;

  if (paramSymbol === params[params.length - 1] && decl && tsm.Node.isParameterDeclaration(decl)) {
    if (decl.isRestParameter()) {
      if (resolvedType?.isTuple()) {
        const tupleElements = resolvedType.getTupleElements();
        if (tupleElements.length === 0) {
          return undefined;
        }
        const restIndex = Math.max(0, argIndex - (params.length - 1));
        return tupleElements[restIndex] ?? tupleElements[tupleElements.length - 1] ?? resolvedType;
      }

      return (
        resolvedType?.getArrayElementType() ?? resolvedType?.getNumberIndexType() ?? resolvedType
      );
    }
  }

  return resolvedType;
}

function getArgumentReplaceRange(
  callExpression: tsm.CallExpression | tsm.NewExpression,
  argIndex: number,
  pos: number,
  rawStart: number,
): { replaceStart: number; replaceEnd: number } {
  const args = callExpression.getArguments();
  let replaceStartAbs = pos + 1;
  let replaceEndAbs = pos + 1;

  if (argIndex < args.length) {
    replaceStartAbs = args[argIndex].getStart();
    replaceEndAbs = args[argIndex].getEnd();
  }

  return {
    replaceStart: Math.max(0, replaceStartAbs - rawStart),
    replaceEnd: Math.max(0, replaceEndAbs - rawStart),
  };
}

function getArgumentPrefixText(
  snippet: tsm.SourceFile,
  argNode: tsm.Expression | undefined,
  pos: number,
): string {
  if (!argNode) return '';

  const start = argNode.getStart();
  if (pos < start) return '';

  const end = Math.min(pos + 1, argNode.getEnd());
  return snippet.getFullText().slice(start, end).trim();
}

type PlaceholderOptions = {
  maxDepth: number;
  maxProperties: number;
  maxVariants: number;
  maxTupleLength: number;
};

const PLACEHOLDER_OPTIONS: PlaceholderOptions = {
  maxDepth: 3,
  maxProperties: 5,
  maxVariants: 8,
  maxTupleLength: 6,
};

const MAX_PLACEHOLDER_ENTRIES = 12;

function buildTypePlaceholders(type: tsm.Type, checker: tsm.TypeChecker): string[] {
  const seen = new Set<tsm.Type>();
  return buildTypePlaceholdersInner(type, checker, PLACEHOLDER_OPTIONS, seen, 0);
}

function getArgumentTypePlaceholders(
  type: tsm.Type,
  checker: tsm.TypeChecker,
  argNode: tsm.Expression | undefined,
): string[] {
  const apparent = type.getApparentType();
  if (!apparent.isUnion() || !argNode || !tsm.Node.isObjectLiteralExpression(argNode)) {
    return buildTypePlaceholders(type, checker);
  }

  const tagged = getTaggedUnionInfo(apparent.getUnionTypes(), checker);
  if (!tagged) {
    return buildTypePlaceholders(type, checker);
  }

  const tagValue = getObjectLiteralTagValue(argNode, tagged.tagName);
  if (!tagValue) {
    return buildTypePlaceholders(type, checker);
  }

  const variant = tagged.variants.find((entry) => entry.tagValue === tagValue);
  if (!variant) {
    return buildTypePlaceholders(type, checker);
  }

  const seen = new Set<tsm.Type>();
  return [
    buildTaggedObjectPlaceholder(
      variant.type,
      tagged.tagName,
      variant.tagValue,
      checker,
      PLACEHOLDER_OPTIONS,
      seen,
      0,
    ),
  ];
}

function buildTypePlaceholdersInner(
  type: tsm.Type,
  checker: tsm.TypeChecker,
  options: PlaceholderOptions,
  seen: Set<tsm.Type>,
  depth: number,
): string[] {
  if (depth > options.maxDepth) return ['?'];
  if (seen.has(type)) return ['?'];

  seen.add(type);
  const literalPlaceholder = getLiteralPlaceholder(type);
  if (literalPlaceholder) {
    seen.delete(type);
    return [literalPlaceholder];
  }

  const primitivePlaceholder = getPrimitivePlaceholder(type);
  if (primitivePlaceholder) {
    seen.delete(type);
    return [primitivePlaceholder];
  }

  const apparent = type.getApparentType();

  if (apparent.isUnion()) {
    const unionTypes = apparent.getUnionTypes();
    const tagged = getTaggedUnionInfo(unionTypes, checker);
    const entries = tagged
      ? tagged.variants.map((variant) =>
          buildTaggedObjectPlaceholder(
            variant.type,
            tagged.tagName,
            variant.tagValue,
            checker,
            options,
            seen,
            depth + 1,
          ),
        )
      : unionTypes.flatMap((unionType) =>
          buildTypePlaceholdersInner(unionType, checker, options, seen, depth + 1),
        );

    const result = uniquePlaceholders(entries).slice(0, options.maxVariants);
    seen.delete(type);
    return result;
  }

  if (apparent.isIntersection()) {
    const intersectionTypes = apparent.getIntersectionTypes();
    if (intersectionTypes.length) {
      const result = buildTypePlaceholdersInner(
        intersectionTypes[0],
        checker,
        options,
        seen,
        depth + 1,
      );
      seen.delete(type);
      return result;
    }
  }

  const arrayElement =
    apparent.getArrayElementType() ??
    type.getArrayElementType() ??
    apparent.getNumberIndexType() ??
    type.getNumberIndexType();
  if (arrayElement || apparent.isArray() || type.isArray()) {
    const elementPlaceholder = arrayElement
      ? (buildTypePlaceholdersInner(arrayElement, checker, options, seen, depth + 1)[0] ?? '?')
      : '?';
    const result = [`[${elementPlaceholder}]`];
    seen.delete(type);
    return result;
  }

  if (apparent.isTuple() || type.isTuple()) {
    const tupleElements = (apparent.isTuple() ? apparent : type)
      .getTupleElements()
      .slice(0, options.maxTupleLength);
    const placeholders = tupleElements.map((elementType) => {
      return buildTypePlaceholdersInner(elementType, checker, options, seen, depth + 1)[0] ?? '?';
    });
    const result = [`[${placeholders.join(', ')}]`];
    seen.delete(type);
    return result;
  }

  if (apparent.getCallSignatures().length) {
    seen.delete(type);
    return ['() => ?'];
  }

  if (isObjectType(apparent)) {
    const result = [buildObjectPlaceholder(apparent, checker, options, seen, depth + 1)];
    seen.delete(type);
    return result;
  }

  seen.delete(type);
  return ['?'];
}

function getLiteralPlaceholder(type: tsm.Type): string | undefined {
  const flags = type.getFlags();
  if (flags & tsm.TypeFlags.StringLiteral) return type.getText();
  if (flags & tsm.TypeFlags.NumberLiteral) return type.getText();
  if (flags & tsm.TypeFlags.BooleanLiteral) return type.getText();
  if (flags & tsm.TypeFlags.BigIntLiteral) return type.getText();
  return undefined;
}

function getPrimitivePlaceholder(type: tsm.Type): string | undefined {
  const flags = type.getFlags();
  if (flags & tsm.TypeFlags.String) return '"?"';
  if (flags & tsm.TypeFlags.Number) return '0';
  if (flags & tsm.TypeFlags.Boolean) return 'false';
  if (flags & tsm.TypeFlags.BigInt) return '0n';
  if (flags & tsm.TypeFlags.Null) return 'null';
  if (flags & tsm.TypeFlags.Undefined) return 'undefined';
  if (flags & tsm.TypeFlags.Any) return '?';
  if (flags & tsm.TypeFlags.Unknown) return '?';
  const symbolName = type.getSymbol()?.getName();
  if (symbolName === 'String') return '"?"';
  if (symbolName === 'Number') return '0';
  if (symbolName === 'Boolean') return 'false';
  if (symbolName === 'BigInt') return '0n';
  return undefined;
}

function isObjectType(type: tsm.Type): boolean {
  return (type.getFlags() & tsm.TypeFlags.Object) !== 0;
}

function buildObjectPlaceholder(
  type: tsm.Type,
  checker: tsm.TypeChecker,
  options: PlaceholderOptions,
  seen: Set<tsm.Type>,
  depth: number,
): string {
  const props = type.getProperties();
  if (!props.length) return '{}';

  const required = props.filter((prop) => (prop.getFlags() & ts.SymbolFlags.Optional) === 0);
  const optional = props.filter((prop) => (prop.getFlags() & ts.SymbolFlags.Optional) !== 0);
  const ordered = [...required, ...optional].slice(0, options.maxProperties);

  const entries = ordered.map((prop) => {
    const name = prop.getName();
    const key = isValidIdentifier(name) ? name : JSON.stringify(name);
    const propType = getSymbolType(prop, checker);
    const placeholder = propType
      ? (buildTypePlaceholdersInner(propType, checker, options, seen, depth)[0] ?? '?')
      : '?';
    return `${key}: ${placeholder}`;
  });

  return `{ ${entries.join(', ')} }`;
}

function buildTaggedObjectPlaceholder(
  type: tsm.Type,
  tagName: string,
  tagValue: string,
  checker: tsm.TypeChecker,
  options: PlaceholderOptions,
  seen: Set<tsm.Type>,
  depth: number,
): string {
  const props = type.getProperties().filter((prop) => prop.getName() !== tagName);
  const required = props.filter((prop) => (prop.getFlags() & ts.SymbolFlags.Optional) === 0);
  const optional = props.filter((prop) => (prop.getFlags() & ts.SymbolFlags.Optional) !== 0);
  const ordered = [...required, ...optional].slice(0, options.maxProperties);

  const entries = ordered.map((prop) => {
    const name = prop.getName();
    const key = isValidIdentifier(name) ? name : JSON.stringify(name);
    const propType = getSymbolType(prop, checker);
    const placeholder = propType
      ? (buildTypePlaceholdersInner(propType, checker, options, seen, depth)[0] ?? '?')
      : '?';
    return `${key}: ${placeholder}`;
  });

  const tagKey = isValidIdentifier(tagName) ? tagName : JSON.stringify(tagName);
  if (!entries.length) {
    return `{ ${tagKey}: ${tagValue} }`;
  }

  return `{ ${tagKey}: ${tagValue}, ${entries.join(', ')} }`;
}

function getTaggedUnionInfo(
  unionTypes: tsm.Type[],
  checker: tsm.TypeChecker,
): { tagName: string; variants: Array<{ type: tsm.Type; tagValue: string }> } | undefined {
  if (!unionTypes.length) return;
  if (!unionTypes.every((type) => isObjectType(type))) return;

  const firstProps = unionTypes[0].getProperties();
  for (const prop of firstProps) {
    const name = prop.getName();
    const variants: Array<{ type: tsm.Type; tagValue: string }> = [];

    for (const variantType of unionTypes) {
      const variantProp = variantType.getProperty(name);
      if (!variantProp) {
        variants.length = 0;
        break;
      }

      const propType = getSymbolType(variantProp, checker);
      const literalValue = propType ? getLiteralPlaceholder(propType) : undefined;
      if (!literalValue) {
        variants.length = 0;
        break;
      }

      variants.push({ type: variantType, tagValue: literalValue });
    }

    if (variants.length === unionTypes.length) {
      return { tagName: name, variants };
    }
  }

  return undefined;
}

function getObjectLiteralTagValue(
  node: tsm.ObjectLiteralExpression,
  tagName: string,
): string | undefined {
  for (const property of node.getProperties()) {
    if (!tsm.Node.isPropertyAssignment(property)) continue;
    if (property.getName() !== tagName) continue;

    const initializer = property.getInitializer();
    if (!initializer) continue;

    switch (initializer.getKind()) {
      case tsm.SyntaxKind.StringLiteral:
      case tsm.SyntaxKind.NumericLiteral:
      case tsm.SyntaxKind.TrueKeyword:
      case tsm.SyntaxKind.FalseKeyword:
        return initializer.getText();
      default:
        return undefined;
    }
  }
  return undefined;
}

function uniquePlaceholders(entries: string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const entry of entries) {
    if (seen.has(entry)) continue;
    seen.add(entry);
    result.push(entry);
  }
  return result;
}

function mergeCompletionEntries(primary: string[], secondary: string[]): string[] {
  return uniquePlaceholders([...primary, ...secondary]);
}

function isValidIdentifier(text: string): boolean {
  if (!text.length) return false;
  if (!ts.isIdentifierStart(text.charCodeAt(0), ts.ScriptTarget.Latest)) return false;
  for (let i = 1; i < text.length; i++) {
    if (!ts.isIdentifierPart(text.charCodeAt(i), ts.ScriptTarget.Latest)) {
      return false;
    }
  }
  return true;
}

function getSymbolType(symbol: tsm.Symbol, checker: tsm.TypeChecker): tsm.Type | undefined {
  const decl = symbol.getDeclarations()[0] ?? symbol.getValueDeclaration();
  if (!decl) return undefined;
  return checker.getTypeAtLocation(decl);
}

function formatDisplayParts(parts: Array<{ text: string; kind: string }>): string {
  return parts.map((p) => colorizePart(p.kind, p.text)).join('');
}

function formatTypeText(typeText: string): string {
  const scanner = ts.createScanner(
    ts.ScriptTarget.Latest,
    false,
    ts.LanguageVariant.Standard,
    typeText,
  );

  const parts: Array<{ text: string; kind: string }> = [];
  let lastPos = 0;

  for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
    const tokenStart = scanner.getTokenStart();
    if (tokenStart > lastPos) {
      const gap = typeText.slice(lastPos, tokenStart);
      parts.push({ text: gap, kind: 'space' });
    }

    const text = scanner.getTokenText();
    parts.push({ text, kind: tokenKindToDisplayPartKind(token) });

    lastPos = scanner.getTokenEnd();
  }

  if (lastPos < typeText.length) {
    parts.push({ text: typeText.slice(lastPos), kind: 'space' });
  }

  return formatDisplayParts(parts);
}

function tokenKindToDisplayPartKind(kind: ts.SyntaxKind): string {
  if (kind === ts.SyntaxKind.Identifier) return 'interfaceName';
  if (kind === ts.SyntaxKind.StringLiteral) return 'stringLiteral';
  if (kind === ts.SyntaxKind.NumericLiteral) return 'numericLiteral';
  if (kind >= ts.SyntaxKind.FirstKeyword && kind <= ts.SyntaxKind.LastKeyword) return 'keyword';

  switch (kind) {
    case ts.SyntaxKind.BarToken:
    case ts.SyntaxKind.AmpersandToken:
    case ts.SyntaxKind.EqualsGreaterThanToken:
      return 'operator';

    case ts.SyntaxKind.DotToken:
    case ts.SyntaxKind.CommaToken:
    case ts.SyntaxKind.ColonToken:
    case ts.SyntaxKind.SemicolonToken:
    case ts.SyntaxKind.QuestionToken:
    case ts.SyntaxKind.OpenParenToken:
    case ts.SyntaxKind.CloseParenToken:
    case ts.SyntaxKind.OpenBracketToken:
    case ts.SyntaxKind.CloseBracketToken:
    case ts.SyntaxKind.OpenBraceToken:
    case ts.SyntaxKind.CloseBraceToken:
    case ts.SyntaxKind.LessThanToken:
    case ts.SyntaxKind.GreaterThanToken:
      return 'punctuation';

    default:
      return 'text';
  }
}

//  enum SymbolDisplayPartKind {
//         aliasName = 0,
//         className = 1,
//         enumName = 2,
//         fieldName = 3,
//         interfaceName = 4,
//         keyword = 5,
//         lineBreak = 6,
//         numericLiteral = 7,
//         stringLiteral = 8,
//         localName = 9,
//         methodName = 10,
//         moduleName = 11,
//         operator = 12,
//         parameterName = 13,
//         propertyName = 14,
//         punctuation = 15,
//         space = 16,
//         text = 17,
//         typeParameterName = 18,
//         enumMemberName = 19,
//         functionName = 20,
//         regularExpressionLiteral = 21,
//         link = 22,
//         linkName = 23,
//         linkText = 24,
//     }
//
function colorizePart(kind: string, text: string): string {
  switch (kind) {
    case 'space':
    case 'text':
      return text;

    case 'lineBreak':
      return '\n';

    case 'keyword':
      return pc.cyan(text);

    case 'stringLiteral':
    case 'numericLiteral':
    case 'regularExpressionLiteral':
      return pc.magenta(text);

    case 'aliasName':
    case 'className':
    case 'enumName':
    case 'interfaceName':
    case 'moduleName':
    case 'typeParameterName':
      return pc.blue(text);

    case 'fieldName':
    case 'propertyName':
    case 'methodName':
    case 'functionName':
    case 'enumMemberName':
      return pc.green(text);

    case 'parameterName':
      return pc.bold(text);

    case 'localName':
      return text;

    case 'link':
    case 'linkName':
    case 'linkText':
      return pc.underline(pc.blue(text));

    case 'punctuation':
    case 'operator':
    case 'comma':
    case 'colon':
    case 'semicolon':
    case 'bracket':
      return pc.dim(text);

    default:
      return text;
  }
}

function getFullExpression(node: tsm.Node): tsm.Node {
  let current = node;

  while (true) {
    const parent = current.getParent();
    if (!parent) break;
    if (parent.getKind() === tsm.SyntaxKind.ExpressionStatement) break;
    current = parent;
  }

  return current;
}

function isPromise(type: tsm.Type): boolean {
  if (type.isUnion()) {
    return type.getUnionTypes().some(isPromise);
  }

  const symbol = type.getSymbol();
  if (!symbol) return false;

  return symbol.getName() === 'Promise';
}

function asLiteralType(type: tsm.Type): string | undefined {
  const flags = type.getFlags();
  if (flags & tsm.TypeFlags.BooleanLiteral) return 'boolean';
  if (flags & tsm.TypeFlags.NumberLiteral) return 'number';
  if (flags & tsm.TypeFlags.StringLiteral) return 'string';
  return undefined;
}

function lastNonWhitespaceIndex(text: string): number {
  for (let i = text.length - 1; i >= 0; i--) {
    if (!/\s/.test(text[i])) return i;
  }
  return -1;
}

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

import { Config } from './config';

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

const SNIPPET_FILE_NAME = '__snippet__.ts';

export class LanguageService {
  private readonly snippetImports;
  private project: tsm.Project;
  private snippetHistory: string;
  private rawSnippet: string;
  private fullSnippetStartPos: number;
  private fullSnippetEndPos: number;

  constructor(config: Config) {
    this.snippetImports =
      Object.entries(config.agents)
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
    this.fullSnippetEndPos = 0;
    this.fullSnippetStartPos = this.snippetImports.length;
  }

  private updateProjectSnippet() {
    const fullSnippet = this.snippetHistory + this.rawSnippet;
    this.fullSnippetEndPos = fullSnippet.trimEnd().length - 1;
    this.fullSnippetStartPos = this.snippetImports.length + this.rawSnippet.search(/\S/);
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
    if (!snippet) {
      return;
    }

    const languageService = this.project.getLanguageService();
    const tsLs = languageService.compilerObject;

    const info = tsLs.getQuickInfoAtPosition(snippet.getFilePath(), this.fullSnippetEndPos);

    if (!info) {
      return;
    }

    let formattedInfo = '';

    if (info.displayParts?.length) {
      formattedInfo += formatDisplayParts(info.displayParts);
    }

    return { formattedInfo };
  }

  getSnippetTypeInfo(): SnippetTypeInfo | undefined {
    const snippet = this.getSnippet();
    if (!snippet) {
      return;
    }

    const node =
      snippet.getDescendantAtPos(this.fullSnippetEndPos) ||
      snippet.getDescendantAtPos(this.fullSnippetStartPos);
    if (!node) {
      return;
    }

    const fullExpressionNode = getFullExpression(node);

    // TODO:
    // console.log({
    //   fullExpressionNodePos: fullExpressionNode.getPos(),
    //   fullSnippetStartPos: this.fullSnippetStartPos,
    //   fullSnippetEndPos: this.fullSnippetEndPos,
    // });

    let nodeType = fullExpressionNode.getType();
    if (!nodeType) {
      return;
    }

    const typeIsPromise = isPromise(nodeType);
    const typeAsLiteralType = typeIsPromise ? undefined : asLiteralType(nodeType);

    // TODO: format the type in the same style as "formattedType" works
    const formattedType = typeAsLiteralType
      ? formatDisplayParts([{ text: typeAsLiteralType, kind: 'keyword' }])
      : this.project.getTypeChecker().getTypeText(nodeType, fullExpressionNode);

    return {
      formattedType,
      isPromise: typeIsPromise,
    };
  }

  getSnippetCompletions() {
    const snippet = this.getSnippet();
    if (!snippet) {
      return {};
    }

    let pos = this.fullSnippetEndPos;

    const triggerCharacter = matchTriggerCharacter(snippet.getText(), pos);
    const triggerKind = triggerCharacter
      ? ts.CompletionTriggerKind.TriggerCharacter
      : ts.CompletionTriggerKind.Invoked;

    const tsLs = this.project.getLanguageService().compilerObject;

    const completions = tsLs.getCompletionsAtPosition(snippet.getFilePath(), pos, {
      triggerKind,
      triggerCharacter,
      includeCompletionsForModuleExports: false,
      includeCompletionsForImportStatements: false,
      includeCompletionsWithInsertText: false,
      includeCompletionsWithSnippetText: false,
    });

    if (!completions) {
      return {};
    }

    let snippetTrimmed = this.rawSnippet.trim();
    for (let entry of completions.entries) {
      if (entry.name.startsWith(snippetTrimmed)) {
        console.log(entry.name);
      }
    }
  }
}

function matchTriggerCharacter(
  text: string,
  pos: number,
): ts.CompletionsTriggerCharacter | undefined {
  const i = Math.max(0, pos - 1);
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

function formatDisplayParts(parts: Array<{ text: string; kind: string }>): string {
  return parts.map((p) => colorizePart(p.kind, p.text)).join('');
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

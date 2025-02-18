import * as monaco from "monaco-editor";

import { AlertCircle, ChevronLeft, Code2, Maximize2, Minimize2, X } from "lucide-react";
import Editor, { Monaco } from "@monaco-editor/react";
import React, { useEffect, useState } from "react";
import { apiClient, baseURL } from "../../lib/api-client";

// Types
interface ContextVariable {
  name: string;
  type: string;
  documentation?: string;
  fields?: Record<string, ContextVariable>;
}

interface RibEditorPanelProps {
  initialValue?: string | null;
  onChange?: (value: string | undefined) => void;
  title?: string;
  summary?: string;
  contextVariables?: ContextVariable[];
  exports?: Record<string, unknown>;
}

interface ValidationErrorProps {
  message: string;
  onClose: () => void;
  onCloseWithError?: () => void;
}
const ValidationError = ({ message, onClose, onCloseWithError }: ValidationErrorProps) => {
  const [hasSeenError, setHasSeenError] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => {
      setHasSeenError(true);
    }, 1500); // Consider error seen after 1.5 seconds
    return () => clearTimeout(timer);
  }, []);

  return (
    <div className="absolute bottom-4 left-4 right-4 z-50">
      <div className="bg-destructive/10 border border-destructive/20 text-destructive px-4 py-3 
                    rounded-lg shadow-lg flex items-center justify-between gap-3 
                    animate-in slide-in-from-bottom">
        <div className="flex items-center gap-2 flex-1">
          <AlertCircle className="h-4 w-4 flex-shrink-0" />
          <span className="text-sm">{message}</span>
        </div>

        <div className="flex items-center gap-2">
          {onCloseWithError && (
            <button
              onClick={onCloseWithError}
              className="px-3 py-1.5 text-xs font-medium rounded-md
                       border border-destructive/20 hover:bg-destructive/20
                       transition-colors"
            >
              Close with Error
            </button>
          )}

          <button
            onClick={onClose}
            className="p-1.5 hover:bg-destructive/10 rounded-md transition-colors"
            aria-label="Dismiss error"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>
    </div>
  );
};

// Language Provider
class RibLanguageProvider {
  private static instance: RibLanguageProvider;
  private contextVariables: ContextVariable[] = [];
  private monaco: Monaco | null = null;
  private disposables: monaco.IDisposable[] = [];

  private constructor() { }

  public static getInstance(): RibLanguageProvider {
    if (!RibLanguageProvider.instance) {
      RibLanguageProvider.instance = new RibLanguageProvider();
    }
    return RibLanguageProvider.instance;
  }

  private findContextVariable(path: string[]): ContextVariable | undefined {
    let current = this.contextVariables.find((v) => v.name === path[0]);
    for (let i = 1; i < path.length && current?.fields; i++) {
      current = current.fields[path[i]];
    }
    return current;
  }

  public configure(monaco: Monaco) {
    this.monaco = monaco;
    this.registerLanguage();
    this.registerProviders();
  }

  private registerLanguage() {
    if (!this.monaco) return;

    this.monaco.languages.register({ id: "rib" });
    this.monaco.languages.setMonarchTokensProvider("rib", {
      keywords: [
        "let",
        "if",
        "then",
        "else",
        "match",
        "for",
        "in",
        "yield",
        "reduce",
        "from",
        "some",
        "none",
        "ok",
        "error",
        "true",
        "false",
      ],
      tokenizer: {
        root: [
          [
            /[a-zA-Z_]\w*/,
            {
              cases: {
                "@keywords": "keyword",
                "@default": "identifier",
              },
            },
          ],
          [/".*?"/, "string"],
          [/\d+u(?:8|16|32|64)/, "number"],
          [/\d+(?:\.\d+)?f?(?:32|64)?/, "number"],
          [/[{}()\[\]]/, "@brackets"],
          [/[;,.]/, "delimiter"],
          [/[=<>!&|+\-*\/]+/, "operator"],
        ],
      },
    });
  }

  private registerProviders() {
    if (!this.monaco) return;

    // Clear existing disposables
    this.dispose();

    // Completion provider
    this.disposables.push(
      this.monaco.languages.registerCompletionItemProvider("rib", {
        provideCompletionItems: (model, position) => {
          const wordUntilPosition = model.getWordUntilPosition(position);
          const range = {
            startLineNumber: position.lineNumber,
            endLineNumber: position.lineNumber,
            startColumn: wordUntilPosition.startColumn,
            endColumn: wordUntilPosition.endColumn,
          };

          const line = model.getLineContent(position.lineNumber);
          const textUntilPosition = line.substring(0, position.column - 1);
          const dotMatch = textUntilPosition.match(
            /([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)\.$/,
          );

          let suggestions: monaco.languages.CompletionItem[] = [];

          if (dotMatch) {
            const pathParts = dotMatch[1].split(".");
            const contextVar = this.findContextVariable(pathParts);

            if (contextVar?.fields) {
              suggestions = Object.values(contextVar.fields).map((field) => ({
                label: field.name,
                kind: monaco.languages.CompletionItemKind.Field,
                detail: field.type,
                documentation: {
                  value: field.documentation || `Type: ${field.type}`,
                },
                insertText: field.name,
                range,
              }));
            }
          } else {
            suggestions = [
              ...this.contextVariables.map((variable) => ({
                label: variable.name,
                kind: monaco.languages.CompletionItemKind.Variable,
                detail: variable.type,
                documentation: {
                  value: variable.documentation || `Type: ${variable.type}`,
                },
                insertText: variable.name,
                range,
              })),
              ...["let", "if", "then", "else", "match", "for", "in"].map(
                (keyword) => ({
                  label: keyword,
                  kind: monaco.languages.CompletionItemKind.Keyword,
                  insertText: keyword,
                  range,
                }),
              ),
            ];
          }

          return { suggestions };
        },
        triggerCharacters: ["."],
      }),
    );

    // Hover provider
    this.disposables.push(
      this.monaco.languages.registerHoverProvider("rib", {
        provideHover: (model, position) => {
          const word = model.getWordAtPosition(position);
          if (!word) return;

          const line = model.getLineContent(position.lineNumber);
          const textUntilPosition = line.substring(0, position.column);
          const identifierMatch = textUntilPosition.match(
            /([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)/,
          );

          if (!identifierMatch) return;

          const fullPath = identifierMatch[1];
          const pathParts = fullPath.split(".");
          const contextVar = this.findContextVariable(pathParts);

          if (contextVar) {
            const contents = [
              {
                value:
                  "```typescript\n" +
                  contextVar.name +
                  ": " +
                  contextVar.type +
                  "\n```",
              },
            ];

            if (contextVar.documentation) {
              contents.push({ value: contextVar.documentation });
            }

            if (contextVar.fields) {
              const fieldsList = Object.values(contextVar.fields)
                .map((field) => `- ${field.name}: ${field.type}`)
                .join("\n");
              contents.push({ value: "\nAvailable fields:\n" + fieldsList });
            }

            return {
              range: {
                startLineNumber: position.lineNumber,
                startColumn: word.startColumn,
                endLineNumber: position.lineNumber,
                endColumn: word.endColumn,
              },
              contents,
            };
          }
        },
      }),
    );
  }

  public updateContextVariables(variables: ContextVariable[]) {
    this.contextVariables = variables;
    this.registerProviders();
  }

  public dispose() {
    this.disposables.forEach((d) => d.dispose());
    this.disposables = [];
  }
}

// Main Component
const RibEditorPanel: React.FC<RibEditorPanelProps> = ({
  initialValue = "",
  onChange,
  title = "Script Editor",
  summary = "Edit your script",
  contextVariables = [],
  exports = {}
}) => {
  const [isOpen, setIsOpen] = useState(false);
  const [currentValue, setCurrentValue] = useState(initialValue);
  const [isMaximized, setIsMaximized] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [isValidating, setIsValidating] = useState(false);

  const handleEditorWillMount = (monaco: Monaco) => {
    const provider = RibLanguageProvider.getInstance();
    provider.configure(monaco);
    provider.updateContextVariables(contextVariables);
  };

  const handleChange = (value: string | undefined) => {
    setCurrentValue(value || "");
    onChange?.(value);
    // Clear any existing validation errors when content changes
    setValidationError(null);
    validateContent();
  };

  const validateContent = async (): Promise<boolean> => {
    setIsValidating(true);
    let origin = window.location.origin;
    let url = baseURL == '/api' ? `${origin}/rib-validator` : baseURL.replace('/api', '') + '/rib-validator';
    try {
      await apiClient.post(url, {
        rib: currentValue,
        exports: exports
      });
      setValidationError(null);
      return true;
    } catch (error: any) {
      setValidationError('Validation failed: ' + error)
      return false;
    } finally {
      setIsValidating(false);
    }
  };

  const handleClose = async () => {
    if (await validateContent()) {
      setIsOpen(false);
    }
  };

  const toggleMaximize = (e: React.MouseEvent) => {
    e.stopPropagation();
    setIsMaximized(!isMaximized);
  };

  useEffect(() => {
    if (isOpen) {
      RibLanguageProvider.getInstance().updateContextVariables(contextVariables);
    }
  }, [contextVariables, isOpen]);

  useEffect(() => {
    return () => {
      if (isOpen) {
        RibLanguageProvider.getInstance().dispose();
      }
    };
  }, [isOpen]);

  return (
    <div className="relative">
      {/* Summary Card */}
      <div
        onClick={() => !isOpen && setIsOpen(true)}
        className="group relative w-full p-4 bg-card border border-border rounded-lg 
                   transition-all duration-200 hover:border-primary/20 cursor-pointer"
      >
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-md bg-primary/10 text-primary">
              <Code2 className="w-4 h-4" />
            </div>
            <div>
              <h3 className="font-medium text-foreground">{title}</h3>
              <p className="text-sm text-muted-foreground">{summary}</p>
            </div>
          </div>
          <button
            onClick={(e) => {
              e.stopPropagation();
              setIsOpen(true);
            }}
            className="p-2 text-muted-foreground hover:text-primary rounded-md
                       opacity-0 group-hover:opacity-100 transition-opacity 
                       hover:bg-primary/10"
          >
            <Code2 className="w-4 h-4" />
          </button>
        </div>
        {currentValue && (
          <div className="mt-3 p-3 bg-card/50 rounded-md border border-border/50">
            <pre className="text-sm font-mono text-muted-foreground overflow-hidden whitespace-pre-wrap line-clamp-2">
              {currentValue}
            </pre>
          </div>
        )}
      </div>

      {/* Editor Panel */}
      {isOpen && (
        <>
          <div
            className={`fixed ${isMaximized ? "inset-4" : "inset-y-12 right-4 w-2/3"
              } bg-card border border-border rounded-lg shadow-2xl 
              transform transition-all duration-300 ease-in-out z-50`}
          >
            <div className="flex items-center justify-between p-3 border-b border-border">
              <div className="flex items-center gap-3">
                <button
                  onClick={handleClose}
                  disabled={isValidating}
                  className="p-2 text-muted-foreground hover:text-primary 
                             rounded-md hover:bg-primary/10 transition-colors
                             disabled:opacity-50"
                >
                  <ChevronLeft className="w-4 h-4" />
                </button>
                <div className="flex items-center gap-2">
                  <Code2 className="w-4 h-4 text-primary" />
                  <span className="text-sm font-medium text-foreground">
                    {title}
                  </span>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <button
                  onClick={toggleMaximize}
                  className="p-2 text-muted-foreground hover:text-primary 
                             rounded-md hover:bg-primary/10 transition-colors"
                >
                  {isMaximized ? (
                    <Minimize2 className="w-4 h-4" />
                  ) : (
                    <Maximize2 className="w-4 h-4" />
                  )}
                </button>
                <button
                  onClick={handleClose}
                  disabled={isValidating}
                  className="p-2 text-muted-foreground hover:text-destructive 
                             rounded-md hover:bg-destructive/10 transition-colors
                             disabled:opacity-50"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>
            </div>

            <div className="relative h-[calc(100%-48px)]">
              <Editor
                height="100%"
                defaultValue={currentValue}
                defaultLanguage="rib"
                theme="vs-dark"
                onChange={handleChange}
                className="z-100"
                beforeMount={handleEditorWillMount}
                options={{
                  minimap: { enabled: false },
                  fontSize: 14,
                  lineNumbers: "on",
                  scrollBeyondLastLine: false,
                  wordWrap: "on",
                  padding: { top: 16, bottom: 16 },
                  automaticLayout: true,
                  suggestOnTriggerCharacters: true,
                  quickSuggestions: true,
                  folding: true,
                  foldingHighlight: true,
                  foldingStrategy: "auto",
                  showFoldingControls: "always",
                  matchBrackets: "always",
                  autoClosingBrackets: "always",
                  autoClosingQuotes: "always",
                  formatOnPaste: true,
                  formatOnType: true,
                }}
              />

              {validationError && (
                <ValidationError
                  message={validationError}
                  onClose={() => setValidationError(null)}
                  onCloseWithError={() => setIsOpen(false)}
                />
              )}
            </div>
          </div>

          {/* Backdrop */}
          <div
            className="fixed inset-0 bg-background/80 backdrop-blur-sm z-40"
            onClick={handleClose}
          />
        </>
      )}
    </div>
  );
};

export default RibEditorPanel;

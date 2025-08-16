// @ts-nocheck
import { useTheme } from "@/components/theme-provider.tsx";
import { cn } from "@/lib/utils";
import Editor, { type EditorProps, useMonaco } from "@monaco-editor/react";
import { forwardRef, useEffect, useState, useRef } from "react";

interface MonacoEditorProps extends EditorProps {
  value?: string;
  language?: string;
  height?: string;
  scriptKeys?: string[];
  suggestVariable?: Record<string, unknown>;
  disabled?: boolean;
  allowExpand?: boolean;
  allowCopy?: boolean;
}

export const RibEditor = forwardRef<HTMLDivElement, MonacoEditorProps>(
  (
    {
      value,
      onChange,
      className,
      scriptKeys,
      suggestVariable,
      disabled = false,
      allowExpand = true,
      _allowCopy = false,
      ...props
    },
    ref,
  ) => {
    const { resolvedTheme } = useTheme();
    const [isFocused, setIsFocused] = useState(false);
    const monacoInstance = useMonaco();
    const monoRef = useRef(null);

    useEffect(() => {
      if (monacoInstance) {
        monacoInstance.languages.register({ id: "rib" });

        monacoInstance.languages.setLanguageConfiguration("rib", {
          comments: {
            lineComment: "//",
            blockComment: ["/*", "*/"],
          },
          brackets: [
            ["{", "}"],
            ["[", "]"],
            ["(", ")"],
          ],
          autoClosingPairs: [
            { open: "{", close: "}" },
            { open: "[", close: "]" },
            { open: "(", close: ")" },
            { open: '"', close: '"' },
            { open: "'", close: "'" },
          ],
          surroundingPairs: [
            { open: "{", close: "}" },
            { open: "[", close: "]" },
            { open: "(", close: ")" },
            { open: '"', close: '"' },
            { open: "'", close: "'" },
          ],
        });

        monacoInstance.languages.setMonarchTokensProvider("rib", {
          defaultToken: "",
          tokenPostfix: ".rib",

          keywords: [
            "let",
            "if",
            "then",
            "else",
            "for",
            "in",
            "yield",
            "reduce",
            "from",
            "true",
            "false",
            "some",
            "none",
            "ok",
            "error",
          ],

          typeKeywords: [
            "bool",
            "s8",
            "u8",
            "s16",
            "u16",
            "s32",
            "u32",
            "s64",
            "u64",
            "f32",
            "f64",
            "char",
            "string",
            "list",
            "tuple",
            "option",
            "result",
          ],

          operators: [
            ">=",
            "<=",
            "==",
            "<",
            ">",
            "&&",
            "||",
            "+",
            "-",
            "*",
            "/",
          ],

          symbols: /[=><!~?:&|+\-*/^%]+/,
          escapes:
            /\\(?:[abfnrtv\\"']|x[0-9A-Fa-f]{1,4}|u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8})/,

          tokenizer: {
            root: [
              // Keywords
              [/\b(if|then|else|for|in|yield|reduce|from|let)\b/, "keyword"],

              // Type keywords
              [
                /\b(bool|s8|u8|s16|u16|s32|u32|s64|u64|f32|f64|char|string|list|tuple|option|result)\b/,
                "type",
              ],

              // Operators
              [/[=><!~?:&|+\-*/^%]+/, "operator"],

              // Namespace (golem:todo)
              [/\b[a-zA-Z_]\w*:\w+\b/, "namespace"],

              // Package (profile)
              [/(?<=:)[a-zA-Z_]\w*(?=\/)/, "package"],

              // Function Name ({update-profile})
              [/\{[\w-]+\}/, "function"],

              // Function Call with String Argument (("input"))
              [/\(\s*".*?"\s*\)/, "string.argument"],

              // Function Call with Parameter (input: string)
              [/\(\s*([\w]+)\s*:/, "parameter"],

              // Parentheses & Operators
              [/[{}()\[\]]/, "@brackets"],
              [/[.:/]/, "operator"],

              { include: "@whitespace" },
              { include: "@numbers" },
              { include: "@strings" },
            ],

            whitespace: [
              [/[ \t\r\n]+/, "white"],
              [/\/\/.*$/, "comment"],
              [/\/\*/, "comment", "@comment"],
            ],

            comment: [
              [/[^/*]+/, "comment"],
              [/\/\*/, "comment", "@push"],
              ["\\*/", "comment", "@pop"],
              [/[/*]/, "comment"],
            ],

            numbers: [
              [/\d*\.\d+([eE][-+]?\d+)?/, "number.float"],
              [/0[xX][0-9a-fA-F]+/, "number.hex"],
              [/\d+/, "number"],
            ],

            strings: [
              [
                /"/,
                { token: "string.quote", bracket: "@open", next: "@string" },
              ],
            ],

            string: [
              [/[^\\"$]+/, "string"],
              [/@escapes/, "string.escape"],
              [/"/, { token: "string.quote", bracket: "@close", next: "@pop" }],
            ],
          },
        });

        monoRef.current =
          monacoInstance.languages.registerCompletionItemProvider("rib", {
            triggerCharacters: [
              ".",
              "r",
              "e",
              "q",
              "u",
              "e",
              "s",
              "t",
              "v",
              "a",
              "r",
            ],

            provideCompletionItems: (model, position) => {
              try {
                const code = model.getValue();

                // Extract local variables
                const variableRegex =
                  /let\s+(\w+)\s*=\s*(\{[\s\S]*?\}|\[[\s\S]*?\]|"[^"]*"|'[^']*'|\d+)/g;
                let localVariables: Record<string, unknown> = {};

                let match;
                while ((match = variableRegex.exec(code)) !== null) {
                  const [_, varName, varValue] = match;
                  try {
                    // Parse the value, handling different types
                    const value =
                      varValue.startsWith("{") || varValue.startsWith("[")
                        ? JSON.parse(varValue.replace(/(\w+):/g, '"$1":'))
                        : varValue.replace(/['"]/g, "");
                    localVariables[varName] = value;
                  } catch {
                    localVariables[varName] = varValue; // Store as string if parsing fails
                  }
                }

                const wordUntilPosition = model.getWordUntilPosition(position);
                const range = {
                  startLineNumber: position.lineNumber,
                  endLineNumber: position.lineNumber,
                  startColumn: wordUntilPosition.startColumn,
                  endColumn: wordUntilPosition.endColumn,
                };

                const getObjectKeys = (
                  obj: unknown,
                  prefix = "",
                ): Array<{
                  label: string;
                  insertText: string;
                  kind: typeof monacoInstance.languages.CompletionItemKind.Property;
                  range: typeof range;
                }> =>
                  Object.entries(obj).flatMap(([key, value]) =>
                    typeof value === "object"
                      ? [
                          {
                            label: prefix + key,
                            insertText: prefix + key,
                            kind: monacoInstance.languages.CompletionItemKind
                              .Property,
                            range,
                          },
                          ...getObjectKeys(value, `${prefix}${key}.`),
                        ]
                      : [
                          {
                            label: prefix + key,
                            insertText: prefix + key,
                            kind: monacoInstance.languages.CompletionItemKind
                              .Property,
                            range,
                          },
                        ],
                  );

                // Get suggestions for each local variable
                const localVariableSuggestions = Object.entries(
                  localVariables,
                ).flatMap(([varName, value]) =>
                  typeof value === "object"
                    ? getObjectKeys(value, `${varName}.`)
                    : [
                        {
                          label: varName,
                          insertText: varName,
                          kind: monacoInstance.languages.CompletionItemKind
                            .Variable,
                          range,
                        },
                      ],
                );

                // Add suggestVariable suggestions
                const suggestVariableSuggestions = suggestVariable
                  ? getObjectKeys(suggestVariable)
                  : [];

                // Combine all suggestions
                const allSuggestions = [
                  ...localVariableSuggestions,
                  ...suggestVariableSuggestions,
                ];

                // Remove duplicates using a Set
                const uniqueSuggestions = Array.from(
                  new Map(
                    allSuggestions.map(suggestion => [
                      `${suggestion.label}:${suggestion.insertText}`, // Use label and insertText as the unique key
                      suggestion,
                    ]),
                  ).values(),
                );
                // Ensure scriptKeys is always an array and filter out invalid values
                const validScriptKeys = (scriptKeys || []).filter(_key => true);

                const functionSuggestions = validScriptKeys.map(fn => ({
                  label: fn,
                  kind: monacoInstance.languages.CompletionItemKind.Function,
                  insertText: fn,
                  detail: "Function",
                  documentation: `Function: ${fn}`,
                  range,
                }));

                return {
                  suggestions: [...uniqueSuggestions, ...functionSuggestions],
                };
              } catch (e) {
                console.error("Error providing completions:", e);
                return { suggestions: [] };
              }
            },
          });
      }
      return () => {
        monoRef?.current?.dispose();
      };
    }, [monacoInstance, scriptKeys, suggestVariable]);

    useEffect(() => {
      if (!monacoInstance) return;
      // DARK MODE THEME
      monacoInstance.editor.defineTheme("rigDarkTheme", {
        base: "vs-dark", // Dark background
        inherit: true,
        rules: [
          { token: "namespace", foreground: "8A2BE2" }, // Purple (Royal Blue Variant)
          { token: "package", foreground: "20B2AA" }, // Teal (Greenish Blue)
          { token: "function", foreground: "FFA500", fontStyle: "bold" }, // Orange (Bold)
          { token: "string.argument", foreground: "FFD700" }, // Light Yellow
          { token: "parameter", foreground: "00BFFF", fontStyle: "italic" }, // Light Blue (Italic)
          { token: "type", foreground: "00FFFF" }, // Bright Cyan
          { token: "operator", foreground: "808080" }, // Gray
          { token: "comment", foreground: "808080", fontStyle: "italic" }, // Light Gray for comments
          { token: "keyword", foreground: "569CD6" }, // Add keyword highlighting
          { token: "type", foreground: "4EC9B0" }, // Add type keyword highlighting
        ],
        colors: {
          "editor.background": "#1E1E1E",
          "editor.foreground": "#D4D4D4",
          "editor.lineHighlightBackground": "#2E2E2E",
          "editorCursor.foreground": "#FFFFFF",
        },
      });

      // LIGHT MODE THEME
      monacoInstance.editor.defineTheme("rigLightTheme", {
        base: "vs",
        inherit: true,
        rules: [
          { token: "namespace", foreground: "8A2BE2" },
          { token: "package", foreground: "20B2AA" },
          { token: "function", foreground: "FFA500", fontStyle: "bold" },
          { token: "string.argument", foreground: "FFD700" },
          { token: "parameter", foreground: "00BFFF", fontStyle: "italic" },
          { token: "type", foreground: "00FFFF" },
          { token: "operator", foreground: "808080" },
          { token: "comment", foreground: "808080", fontStyle: "italic" },
          { token: "keyword", foreground: "0000FF" }, // Add keyword highlighting
          { token: "type", foreground: "008000" }, // Add type keyword highlighting
        ],
        colors: {
          "editor.background": "#FFFFFF",
          "editor.foreground": "#333333",
          "editor.lineHighlightBackground": "#F0F0F0",
          "editorCursor.foreground": "#000000",
        },
      });
      setRigEditorTheme(resolvedTheme === "dark");
    }, [resolvedTheme, monacoInstance]);

    function setRigEditorTheme(isDarkMode: boolean) {
      if (!monacoInstance) return;
      monacoInstance.editor.setTheme(
        isDarkMode ? "rigDarkTheme" : "rigLightTheme",
      );
    }

    return (
      <div
        ref={ref}
        className={cn(
          "relative rounded-md border p-2 transition-all duration-200 ease-in-out",
          isFocused && allowExpand ? "h-[300px] border-primary" : "h-[100px]",
          className,
        )}
      >
        <Editor
          value={value}
          onChange={onChange}
          language={"rib"}
          theme={resolvedTheme === "dark" ? "rigDarkTheme" : "rigLightTheme"}
          options={{
            minimap: { enabled: false },
            scrollBeyondLastLine: false,
            fontSize: 14,
            folding: false,
            lineNumbers: "off",
            lineNumbersMinChars: 1,
            lineDecorationsWidth: 0,
            renderLineHighlight: "none",
            glyphMargin: true,
            readOnly: disabled,
            wordWrap: "on",
            scrollbar: {
              vertical: "hidden",
              horizontal: "hidden",
            },
            padding: {
              top: 5,
            },
          }}
          onMount={(editor, _monaco) => {
            editor.onDidFocusEditorWidget(() => setIsFocused(true));
            editor.onDidBlurEditorWidget(() => setIsFocused(false));

            setTimeout(() => {
              const suggestWidget = document.querySelector(".suggest-widget");
              if (suggestWidget) {
                (suggestWidget as HTMLElement).style.zIndex = "10000";
              }
            }, 500);
          }}
          {...props}
        />
      </div>
    );
  },
);

RibEditor.displayName = "RibEditor";

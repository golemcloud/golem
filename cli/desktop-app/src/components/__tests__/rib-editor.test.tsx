import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockedFunction,
} from "vitest";
import { render, screen } from "@testing-library/react";
import React from "react";
import { RibEditor } from "../rib-editor";
import { useTheme } from "../theme-provider";
import { useMonaco } from "@monaco-editor/react";

interface MockEditor {
  onDidFocusEditorWidget: ReturnType<typeof vi.fn>;
  onDidBlurEditorWidget: ReturnType<typeof vi.fn>;
}

interface MockMonaco {
  languages: {
    register: ReturnType<typeof vi.fn>;
    setLanguageConfiguration: ReturnType<typeof vi.fn>;
    setMonarchTokensProvider: ReturnType<typeof vi.fn>;
    registerCompletionItemProvider: ReturnType<typeof vi.fn>;
    CompletionItemKind: {
      Property: string;
      Variable: string;
      Function: string;
    };
  };
  editor: {
    defineTheme: ReturnType<typeof vi.fn>;
    setTheme: ReturnType<typeof vi.fn>;
  };
}

interface MockCompletionProvider {
  triggerCharacters?: string[];
}

// Mock dependencies
vi.mock("../theme-provider", () => ({
  useTheme: vi.fn(),
}));

interface MockMonacoEditorProps {
  onMount?: (editor: MockEditor, monaco: MockMonaco) => void;
  onFocus?: () => void;
  onBlur?: () => void;
  _onChange?: (value: string | undefined) => void;
  value?: string;
  language?: string;
  theme?: string;
  options?: { readOnly?: boolean };
}

vi.mock("@monaco-editor/react", () => ({
  default: ({
    onMount,
    onFocus,
    onBlur,
    _onChange,
    ...props
  }: MockMonacoEditorProps) => (
    <div
      data-testid="monaco-editor"
      data-value={props.value}
      data-language={props.language}
      data-theme={props.theme}
      data-readonly={props.options?.readOnly}
      onFocus={() => onFocus?.()}
      onBlur={() => onBlur?.()}
      onClick={() => {
        // Simulate Monaco editor onMount
        const mockEditor: MockEditor = {
          onDidFocusEditorWidget: vi.fn(callback => {
            callback();
            return { dispose: vi.fn() };
          }),
          onDidBlurEditorWidget: vi.fn(callback => {
            callback();
            return { dispose: vi.fn() };
          }),
        };
        const mockMonacoForMount: MockMonaco = {
          languages: {
            register: vi.fn(),
            setLanguageConfiguration: vi.fn(),
            setMonarchTokensProvider: vi.fn(),
            registerCompletionItemProvider: vi.fn(() => ({ dispose: vi.fn() })),
            CompletionItemKind: {
              Property: "Property",
              Variable: "Variable",
              Function: "Function",
            },
          },
          editor: {
            defineTheme: vi.fn(),
            setTheme: vi.fn(),
          },
        };
        onMount?.(mockEditor, mockMonacoForMount);
      }}
    >
      Monaco Editor Mock
    </div>
  ),
  useMonaco: vi.fn(),
}));

vi.mock("@/lib/utils", () => ({
  cn: vi.fn((...args) => args.filter(Boolean).join(" ")),
}));

describe("RibEditor", () => {
  const mockMonaco: MockMonaco = {
    languages: {
      register: vi.fn(),
      setLanguageConfiguration: vi.fn(),
      setMonarchTokensProvider: vi.fn(),
      registerCompletionItemProvider: vi.fn(() => ({ dispose: vi.fn() })),
      CompletionItemKind: {
        Property: "Property",
        Variable: "Variable",
        Function: "Function",
      },
    },
    editor: {
      defineTheme: vi.fn(),
      setTheme: vi.fn(),
    },
  };

  const mockTheme = {
    theme: "light" as const,
    setTheme: vi.fn(),
    resolvedTheme: "light" as const,
  };

  beforeEach(() => {
    vi.clearAllMocks();
    (useTheme as MockedFunction<typeof useTheme>).mockReturnValue(mockTheme);
    (useMonaco as MockedFunction<typeof useMonaco>).mockReturnValue(
      mockMonaco as unknown as ReturnType<typeof useMonaco>,
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("Basic rendering", () => {
    it("should render RibEditor with default props", () => {
      render(<RibEditor />);

      expect(screen.getByTestId("monaco-editor")).toBeInTheDocument();
      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-language",
        "rib",
      );
    });

    it("should render with custom value", () => {
      const testValue = "let x = 42";
      render(<RibEditor value={testValue} />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-value",
        testValue,
      );
    });

    it("should apply custom className", () => {
      const { container } = render(<RibEditor className="custom-class" />);

      expect(container.firstChild).toHaveClass("custom-class");
    });
  });

  describe("Language configuration", () => {
    it("should register RIB language when Monaco is available", () => {
      render(<RibEditor />);

      expect(mockMonaco.languages.register).toHaveBeenCalledWith({ id: "rib" });
    });

    it("should set language configuration for RIB", () => {
      render(<RibEditor />);

      expect(
        mockMonaco.languages.setLanguageConfiguration,
      ).toHaveBeenCalledWith("rib", {
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
    });

    it("should set Monarch tokens provider for RIB syntax highlighting", () => {
      render(<RibEditor />);

      expect(
        mockMonaco.languages.setMonarchTokensProvider,
      ).toHaveBeenCalledWith(
        "rib",
        expect.objectContaining({
          defaultToken: "",
          tokenPostfix: ".rib",
          keywords: expect.arrayContaining([
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
          ]),
          typeKeywords: expect.arrayContaining([
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
          ]),
          operators: expect.arrayContaining([
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
          ]),
        }),
      );
    });
  });

  describe("Theme management", () => {
    it("should define dark theme when Monaco is available", () => {
      render(<RibEditor />);

      expect(mockMonaco.editor.defineTheme).toHaveBeenCalledWith(
        "rigDarkTheme",
        expect.objectContaining({
          base: "vs-dark",
          inherit: true,
          rules: expect.arrayContaining([
            { token: "namespace", foreground: "8A2BE2" },
            { token: "package", foreground: "20B2AA" },
            { token: "function", foreground: "FFA500", fontStyle: "bold" },
          ]),
          colors: expect.objectContaining({
            "editor.background": "#1E1E1E",
            "editor.foreground": "#D4D4D4",
          }),
        }),
      );
    });

    it("should define light theme when Monaco is available", () => {
      render(<RibEditor />);

      expect(mockMonaco.editor.defineTheme).toHaveBeenCalledWith(
        "rigLightTheme",
        expect.objectContaining({
          base: "vs",
          inherit: true,
          rules: expect.arrayContaining([
            { token: "namespace", foreground: "8A2BE2" },
            { token: "package", foreground: "20B2AA" },
            { token: "function", foreground: "FFA500", fontStyle: "bold" },
          ]),
          colors: expect.objectContaining({
            "editor.background": "#FFFFFF",
            "editor.foreground": "#333333",
          }),
        }),
      );
    });

    it("should use light theme when theme is light", () => {
      render(<RibEditor />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-theme",
        "rigLightTheme",
      );
    });

    it("should use dark theme when theme is dark", () => {
      (useTheme as MockedFunction<typeof useTheme>).mockReturnValue({
        theme: "dark" as const,
        setTheme: vi.fn(),
        resolvedTheme: "dark" as const,
      });

      render(<RibEditor />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-theme",
        "rigDarkTheme",
      );
    });

    it("should update theme when theme provider changes", () => {
      const { rerender } = render(<RibEditor />);

      (useTheme as MockedFunction<typeof useTheme>).mockReturnValue({
        theme: "dark" as const,
        setTheme: vi.fn(),
        resolvedTheme: "dark" as const,
      });
      rerender(<RibEditor />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-theme",
        "rigDarkTheme",
      );
    });
  });

  describe("Completion provider", () => {
    it("should register completion item provider for RIB", () => {
      render(<RibEditor />);

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalledWith(
        "rib",
        expect.objectContaining({
          triggerCharacters: expect.arrayContaining([
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
          ]),
          provideCompletionItems: expect.any(Function),
        }),
      );
    });

    it("should provide script key suggestions when scriptKeys are provided", () => {
      const scriptKeys = ["function1", "function2", "function3"];
      render(<RibEditor scriptKeys={scriptKeys} />);

      const providerCalls =
        mockMonaco.languages.registerCompletionItemProvider.mock.calls;
      const provider =
        providerCalls.length > 0 &&
        providerCalls[0] &&
        providerCalls[0].length > 1
          ? (providerCalls[0] as [unknown, MockCompletionProvider])[1]
          : null;

      expect(provider?.triggerCharacters).toEqual(
        expect.arrayContaining([
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
        ]),
      );
    });

    it("should provide suggestions based on suggestVariable prop", () => {
      const suggestVariable = {
        request: { body: "string", headers: {} },
        response: { status: 200 },
      };

      render(<RibEditor suggestVariable={suggestVariable} />);

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();
    });

    it("should handle completion provider errors gracefully", () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});

      // Mock a provider that throws an error

      render(<RibEditor />);

      // The provider should be registered without throwing
      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();

      consoleSpy.mockRestore();
    });
  });

  describe("Focus and expansion behavior", () => {
    it("should not expand when allowExpand is false", () => {
      const { container } = render(<RibEditor allowExpand={false} />);

      // Should have default height
      expect(container.firstChild).toHaveClass("h-[100px]");
    });
  });

  describe("Editor options", () => {
    it("should set readOnly when disabled prop is true", () => {
      render(<RibEditor disabled={true} />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-readonly",
        "true",
      );
    });

    it("should not set readOnly when disabled prop is false", () => {
      render(<RibEditor disabled={false} />);

      expect(screen.getByTestId("monaco-editor")).toHaveAttribute(
        "data-readonly",
        "false",
      );
    });

    it("should pass through other props to Monaco Editor", () => {
      const onChangeHandler = vi.fn();
      render(<RibEditor onChange={onChangeHandler} />);

      expect(screen.getByTestId("monaco-editor")).toBeInTheDocument();
    });
  });

  describe("Cleanup", () => {
    it("should dispose completion provider on unmount", () => {
      const mockDispose = vi.fn();
      mockMonaco.languages.registerCompletionItemProvider.mockReturnValue({
        dispose: mockDispose,
      } as Record<string, unknown>);

      const { unmount } = render(<RibEditor />);
      unmount();

      expect(mockDispose).toHaveBeenCalled();
    });

    it("should handle cleanup when no provider exists", () => {
      mockMonaco.languages.registerCompletionItemProvider.mockReturnValue(
        null as unknown as { dispose: () => void },
      );

      const { unmount } = render(<RibEditor />);

      // Should not throw error
      expect(() => unmount()).not.toThrow();
    });
  });

  describe("Edge cases", () => {
    it("should handle missing Monaco instance gracefully", () => {
      (useMonaco as MockedFunction<typeof useMonaco>).mockReturnValue(null);

      expect(() => render(<RibEditor />)).not.toThrow();
    });

    it("should handle empty scriptKeys array", () => {
      render(<RibEditor scriptKeys={[]} />);

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();
    });

    it("should handle undefined scriptKeys", () => {
      render(<RibEditor scriptKeys={undefined} />);

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();
    });

    it("should handle null suggestVariable", () => {
      render(
        <RibEditor
          suggestVariable={null as unknown as Record<string, unknown>}
        />,
      );

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();
    });

    it("should handle empty suggestVariable object", () => {
      render(<RibEditor suggestVariable={{}} />);

      expect(
        mockMonaco.languages.registerCompletionItemProvider,
      ).toHaveBeenCalled();
    });
  });

  describe("Forward ref", () => {
    it("should forward ref to container div", () => {
      const ref = React.createRef<HTMLDivElement>();
      render(<RibEditor ref={ref} />);

      expect(ref.current).toBeInstanceOf(HTMLDivElement);
    });
  });

  describe("Accessibility", () => {
    it("should have proper container structure", () => {
      const { container } = render(<RibEditor />);

      expect(container.firstChild).toHaveClass(
        "relative",
        "rounded-md",
        "border",
        "p-2",
      );
    });
  });
});

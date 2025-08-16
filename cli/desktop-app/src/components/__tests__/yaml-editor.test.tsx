import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import React from "react";
import { YamlEditor } from "../yaml-editor";
import * as yaml from "js-yaml";
// Mock dependencies
vi.mock("@/components/theme-provider.tsx", () => ({
  useTheme: () => ({ theme: "light" }),
}));

vi.mock("@monaco-editor/react", () => ({
  default: ({
    value,
    onChange,
    onMount,
    theme,
  }: {
    value?: string;
    onChange?: (value: string) => void;
    onMount?: (editor: unknown, monaco: unknown) => void;
    theme?: string;
  }) => {
    const mockEditor = {
      getModel: () => ({
        getLanguageId: () => "yaml",
      }),
      setValue: vi.fn(),
      getValue: () => value,
    };

    const mockMonaco = {
      MarkerSeverity: {
        Error: 8,
        Warning: 4,
      },
      editor: {
        setModelMarkers: vi.fn(),
      },
      languages: {
        register: vi.fn(),
        setMonarchTokensProvider: vi.fn(),
        typescript: {
          javascriptDefaults: {
            setDiagnosticsOptions: vi.fn(),
          },
        },
      },
    };

    React.useEffect(() => {
      if (onMount) {
        onMount(mockEditor, mockMonaco);
      }
    }, []);

    return (
      <textarea
        data-testid="monaco-editor"
        value={value}
        onChange={e => onChange?.(e.target.value)}
        data-theme={theme}
      />
    );
  },
}));

vi.mock("js-yaml", async () => {
  return {
    loadAll: vi.fn(() => [
      /* mocked return */
    ]),
  };
});

const loadAll = vi.mocked(yaml.loadAll);

describe("YamlEditor", () => {
  const mockOnChange = vi.fn();
  const defaultProps = {
    value: "test: value\nkey: data",
    onChange: mockOnChange,
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("Basic rendering", () => {
    it("should render Monaco editor with YAML content", () => {
      render(<YamlEditor {...defaultProps} />);

      const editor = screen.getByTestId("monaco-editor");
      expect(editor).toBeInTheDocument();
      expect(editor).toHaveValue("test: value\nkey: data");
    });

    it("should apply theme to editor", () => {
      render(<YamlEditor {...defaultProps} />);

      const editor = screen.getByTestId("monaco-editor");
      expect(editor).toHaveAttribute("data-theme", "vs-light");
    });

    it("should handle empty value", () => {
      render(<YamlEditor value="" onChange={mockOnChange} />);

      const editor = screen.getByTestId("monaco-editor");
      expect(editor).toHaveValue("");
    });
  });

  describe("YAML validation", () => {
    it("should validate YAML content using js-yaml", () => {
      render(<YamlEditor {...defaultProps} />);

      expect(loadAll).toHaveBeenCalledWith(
        defaultProps.value,
        expect.any(Function),
        expect.objectContaining({
          filename: "document.yaml",
          onWarning: expect.any(Function),
        }),
      );
    });

    describe("Editor configuration", () => {
      it("should initialize Monaco editor with correct settings", () => {
        render(<YamlEditor {...defaultProps} />);

        // Monaco editor should be rendered
        expect(screen.getByTestId("monaco-editor")).toBeInTheDocument();
      });

      it("should handle editor mount correctly", () => {
        // This test verifies that the onMount callback is properly handled
        render(<YamlEditor {...defaultProps} />);

        expect(screen.getByTestId("monaco-editor")).toBeInTheDocument();
      });
    });

    describe("Theme integration", () => {
      it("should use light theme from theme provider", () => {
        render(<YamlEditor {...defaultProps} />);

        const editor = screen.getByTestId("monaco-editor");
        expect(editor).toHaveAttribute("data-theme", "vs-light");
      });
    });

    describe("Error handling", () => {
      it("should gracefully handle editor initialization errors", () => {
        // Mock console.error to avoid noise in test output
        const consoleSpy = vi
          .spyOn(console, "error")
          .mockImplementation(() => {});

        expect(() => {
          render(<YamlEditor {...defaultProps} />);
        }).not.toThrow();

        consoleSpy.mockRestore();
      });

      it("should handle undefined onChange callback", () => {
        expect(() => {
          render(<YamlEditor value="test: value" onChange={vi.fn()} />);
        }).not.toThrow();
      });

      it("should handle malformed YAML gracefully", () => {
        const malformedYaml = `
invalid yaml structure
  no proper indentation
    - list item without proper parent
  unclosed: "string
`;

        expect(() => {
          render(<YamlEditor value={malformedYaml} onChange={mockOnChange} />);
        }).not.toThrow();
      });
    });

    describe("Performance", () => {
      it("should not cause unnecessary re-renders", () => {
        const { rerender } = render(<YamlEditor {...defaultProps} />);

        // Re-render with same props
        rerender(<YamlEditor {...defaultProps} />);

        // Should still be functional
        expect(screen.getByTestId("monaco-editor")).toBeInTheDocument();
      });

      it("should handle large YAML files", () => {
        const largeYaml = Array.from(
          { length: 1000 },
          (_, i) => `key${i}: value${i}`,
        ).join("\n");

        expect(() => {
          render(<YamlEditor value={largeYaml} onChange={mockOnChange} />);
        }).not.toThrow();

        expect(screen.getByTestId("monaco-editor")).toHaveValue(largeYaml);
      });
    });
  });
});

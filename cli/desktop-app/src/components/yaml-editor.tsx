// @ts-nocheck
import { useTheme } from "@/components/theme-provider.tsx";
import Editor, { type Monaco, type OnMount } from "@monaco-editor/react";
import * as yaml from "js-yaml";
import { useRef, useState } from "react";

interface YamlEditorProps {
  value: string;
  onChange: (value: string) => void;
}

export function YamlEditor({ value, onChange }: YamlEditorProps) {
  const monacoRef = useRef<Monaco | null>(null);
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const [markers, setMarkers] = useState<
    {
      severity: string;
      startLineNumber: number;
      startColumn: number;
      endLineNumber: number;
      endColumn: number;
      message: string;
    }[]
  >([]);
  const { resolvedTheme } = useTheme();

  const validateYaml = (content: string, monaco: Monaco) => {
    if (!editorRef.current) return;

    const model = editorRef.current.getModel();
    if (!model) return;

    const markers: monaco.editor.IMarkerData[] = [];

    try {
      yaml.loadAll(
        content,
        _doc => {
          // This function will be called for each document in the YAML file
        },
        {
          filename: "document.yaml",
          onWarning: error => {
            markers.push(createMarker(error, monaco, "Warning"));
          },
        },
      );
    } catch (e) {
      if (e instanceof yaml.YAMLException) {
        markers.push(createMarker(e, monaco, "Error"));
      }
    }

    // Additional custom validations
    const lines = content.split("\n");
    lines.forEach((line, index) => {
      if (line.trim().startsWith("- ") && line.trim().length === 2) {
        markers.push({
          severity: monaco.MarkerSeverity.Warning,
          startLineNumber: index + 1,
          startColumn: 1,
          endLineNumber: index + 1,
          endColumn: line.length + 1,
          message: "Empty list item",
        });
      }
      if (line.includes("\t")) {
        markers.push({
          severity: monaco.MarkerSeverity.Warning,
          startLineNumber: index + 1,
          startColumn: 1,
          endLineNumber: index + 1,
          endColumn: line.length + 1,
          message: "Use spaces for indentation instead of tabs",
        });
      }
    });
    setMarkers(markers);
    monaco.editor.setModelMarkers(model, "yaml", markers);
  };

  const createMarker = (
    error: yaml.YAMLException,
    monaco: Monaco,
    severity: "Error" | "Warning",
  ) => {
    const line = error.mark ? error.mark.line + 1 : 1;
    const column = error.mark ? error.mark.column + 1 : 1;
    return {
      severity:
        severity === "Error"
          ? monaco.MarkerSeverity.Error
          : monaco.MarkerSeverity.Warning,
      startLineNumber: line,
      startColumn: column,
      endLineNumber: line,
      endColumn: column + 1,
      message: error.reason || error.message,
    };
  };

  const handleEditorChange = (value: string | undefined) => {
    if (!value) return;
    onChange(value);
    if (monacoRef.current) {
      validateYaml(value, monacoRef.current);
    }
  };

  const handleEditorDidMount: OnMount = (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    // Set initial validation state
    validateYaml(value, monaco);

    // Configure Monaco YAML settings
    monaco.languages.typescript.javascriptDefaults.setDiagnosticsOptions({
      noSemanticValidation: true,
      noSyntaxValidation: true,
    });

    // Set up YAML language configuration
    monaco.languages.register({ id: "yaml" });
    monaco.languages.setMonarchTokensProvider("yaml", {
      tokenizer: {
        root: [
          [/^[\t ]*[A-Za-z_$][\w$]*:/, "key"],
          [/^[\t ]*-/, "delimiter"],
          [/".*?"/, "string"],
          [/'.*?'/, "string"],
          [/\d+/, "number"],
          [/\btrue\b|\bfalse\b/, "boolean"],
          [/\bnull\b/, "null"],
          [/#.*$/, "comment"],
        ],
      },
    });
  };

  return (
    <div className="relative h-[400px] border rounded-md overflow-hidden">
      <Editor
        defaultLanguage="yaml"
        value={value}
        theme={resolvedTheme === "dark" ? "vs-dark" : "vs-light"}
        onChange={handleEditorChange}
        onMount={handleEditorDidMount}
        options={{
          minimap: { enabled: false },
          lineNumbers: "off",
          roundedSelection: false,
          scrollBeyondLastLine: false,
          readOnly: false,
          wordWrap: "on",
          automaticLayout: true,
          tabSize: 2,
          insertSpaces: true,
        }}
      />
      {markers && (
        <div className="text-sm text-destructive bg-destructive/10 p-3 rounded-md">
          {markers.map(x => x.message).join(" \n")}
        </div>
      )}
    </div>
  );
}

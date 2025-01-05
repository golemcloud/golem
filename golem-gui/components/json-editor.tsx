import React, { useRef } from "react";
import { Editor } from "@monaco-editor/react";
import { editor as MonacoEditor } from "monaco-editor";
import { useTheme } from "next-themes"


const JsonEditor = ({ json }: {json:Record<string,unknown>|Array<unknown>|string}) => {
  const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor|null>(null);
  const {theme} = useTheme()

  const handleEditorDidMount = ( editor: MonacoEditor.IStandaloneCodeEditor) => {
    editorRef.current = editor;
    // Trigger format document action immediately after mount
    editor?.getAction("editor.action.formatDocument")?.run();
  };

  return (
    <div
      style={{
        height: "50vh",
        overflow: "auto",
        borderRadius: "4px",
      }}
      className="border"
    >
      <Editor
        defaultLanguage="json"
        defaultValue={typeof json ==="string" ? json : JSON.stringify(json)}
        options={{
          cursorStyle: "line",
          formatOnPaste: true,
          formatOnType: true,
          wordWrap: "on",
          scrollBeyondLastLine: false,
        }}
        theme={theme == "dark" ? "vs-dark" : "light"  }
        onMount={handleEditorDidMount}
      />
    </div>
  );
};

export default JsonEditor;

import React, { useRef, useState } from "react";
import { FileUp } from "lucide-react";
import ErrorBoundary from "@/components/errorBoundary";

const WasmUpload = () => {
  const [file, setFile] = useState<File | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const selectedFile = e.target.files?.[0];
    if (selectedFile && selectedFile.size <= 50 * 1024 * 1024) {
      // 50MB limit
      setFile(selectedFile);
    }
  };

  return (
    <ErrorBoundary>
      <div>
        <div className="grid grid-cols-[1fr_auto] items-center gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              WASM Binary
            </label>
            <p className="text-sm text-gray-600 mb-3">
              The compiled WASM binary of your component.
            </p>
          </div>
          {file && (
            <div
              className="grid items-center justify-center hover:bg-gray-50 cursor-pointer"
              onClick={() => setFile(null)}
            >
              Clear
            </div>
          )}
        </div>
        <div
          className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"
          onClick={() => fileInputRef.current?.click()}
        >
          <div className="flex flex-col items-center justify-center text-center">
            <FileUp className="h-8 w-8 text-gray-400 mb-3" />
            <p className="font-medium mb-1">
              {file ? file.name : "Upload Component WASM"}
            </p>
            <p className="text-sm text-gray-500 mb-4">File up to 50MB</p>
            <input
              ref={fileInputRef}
              type="file"
              accept="application/wasm,.wasm"
              onChange={handleFileSelect}
              className="hidden"
            />
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
};

export default WasmUpload;

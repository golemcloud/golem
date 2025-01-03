import { useParams } from "react-router-dom";

import ComponentLeftNav from "./componentsLeftNav";
import WasmUpload from "../create/wasmUpload.tsx";
import FileManager from "../create/fileManager.tsx";
import ErrorBoundary from "@/components/errorBoundary.tsx";

export default function ComponentUpdate() {
  const { componentId } = useParams();

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {componentId}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="max-w-4xl mx-auto p-6">
              <h1 className="text-3xl font-semibold mb-2">Update Component</h1>
              <p className="text-gray-500 text-lg mb-8">
                Update component version
              </p>
              <WasmUpload />
              <FileManager />
              <div className="flex justify-end">
                <button className="flex items-center space-x-2 bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
                  <span>Update</span>
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}

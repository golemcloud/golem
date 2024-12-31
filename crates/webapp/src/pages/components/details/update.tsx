import { useParams } from "react-router-dom";

import ComponentLeftNav from "./componentsLeftNav";
import WASMUpload from "../create/WASMUpload";
import FileManager from "../create/FileManager";

export default function ComponentUpdate() {
  const { componentId } = useParams();

  return (
    <div className="flex">
      <ComponentLeftNav />
      <div className="flex-1 p-8">
        <div className="flex items-center justify-between mb-8">
          <div className="grid grid-cols-2 gap-4">
            <h1 className="text-2xl font-semibold mb-2">{componentId}</h1>
            <div className="flex items-center gap-2">
              <span className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border border-primary-border w-fit font-mono">
                0.1.0
              </span>
            </div>
          </div>
        </div>
        <div className="max-w-4xl mx-auto p-6">
          <h1 className="text-3xl font-semibold mb-2">Update Component</h1>
          <p className="text-gray-500 text-lg mb-8">Update component version</p>
          <WASMUpload />
          <FileManager />
          <div className="flex justify-end">
            <button className="flex items-center space-x-2 bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
              <span>Update</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

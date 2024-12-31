/* eslint-disable @typescript-eslint/no-unused-vars */
import React, {useState} from "react";
import { Search, LayoutGrid, Plus } from "lucide-react";
import { useNavigate } from "react-router-dom";
const Components = () => {
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState([{ name: "component1", version: "0.1.0", id: "component1" }]);

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex items-center justify-between gap-4 mb-8">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5" />
          <input
            type="text"
            placeholder="Search Components..."
            className="w-full pl-10 pr-4 py-2 border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => navigate("/components/create")}
            className="flex items-center gap-2 bg-blue-600 text-white px-4 py-2 rounded-lg hover:bg-blue-700"
          >
            <span>New</span>
            <Plus className="h-5 w-5" />
          </button>
        </div>
      </div>

      {componentList.length === 0 ? (
        <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
          <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
            <LayoutGrid className="h-8 w-8 text-gray-400" />
          </div>
          <h2 className="text-xl font-semibold mb-2">No Project Components</h2>
          <p className="text-gray-500 mb-6">
            Create a new component to get started.
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-3 gap-6">
        {componentList.map(({ name, id }: { name: string; id: string }) => ( 
        <div
          key={id}
          className="bg-white rounded-lg border border-gray-200 p-6 hover:shadow-md transition-shadow cursor-pointer"
          onClick={() => navigate(`/components/${id}`)}
        >
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-medium">{name}</h3>
          </div>
        </div>))}
        </div>
      )}
    </div>
  );
};

export default Components;

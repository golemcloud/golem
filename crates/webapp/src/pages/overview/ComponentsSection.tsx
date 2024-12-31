import React from 'react';
import { Layers } from 'lucide-react';
import { useNavigate } from 'react-router-dom';


const ComponentsSection = () => {
  const navigate = useNavigate();
  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">Components</h2>
        <button className="text-blue-600 hover:text-blue-700" onClick={() =>{
          navigate("/components");
        }}>View All</button>
      </div>
      <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-gray-200 rounded-lg">
        <Layers className="h-12 w-12 text-gray-400 mb-4" />
        <h3 className="text-lg font-medium mb-2">No Project Components</h3>
        <p className="text-gray-500 mb-4">Create your first component to get started</p>
        <button className="flex items-center space-x-2 bg-blue-600 text-white px-4 py-2 rounded-md hover:bg-blue-700" onClick={() =>{
          navigate("/components/create");
        }}>
          <span>Create New</span>
        </button>
      </div>
    </div>
  );
};

export default ComponentsSection;
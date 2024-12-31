import React from 'react';
import { useNavigate } from 'react-router-dom';


const APISection = () => {
  const navigate = useNavigate();
  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">APIs</h2>
        <button className="text-blue-600 hover:text-blue-700" onClick={() =>{
          navigate("/apis");
        }}>View All</button>
      </div>
      <button className="flex w-full items-center justify-between py-2 px-4 bg-gray-50 rounded" onClick={() =>{
          navigate("/apis/vvvvv");
        }}>
        <span className="text-gray-700">vvvvv</span>
        <span className="text-gray-500 text-sm">0.1.0</span>
      </button>
    </div>
  );
};

export default APISection;
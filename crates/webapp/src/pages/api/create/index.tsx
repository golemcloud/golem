import React, { useState } from 'react';
import { useNavigate } from 'react-router-dom';

const CreateAPI = () => {
  const navigate = useNavigate();
  const [apiName, setApiName] = useState('');
  const [version, setVersion] = useState('0.1.0');

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    // Handle API creation
    navigate(`/apis/${apiName}`);
  };

  return (
    <div className="container mx-auto px-4 py-8 max-w-2xl">
      <h1 className="text-2xl font-semibold mb-2">Create a new API</h1>
      <p className="text-gray-600 mb-8">Export worker functions as a REST API</p>

      <form onSubmit={handleSubmit} className="space-y-6">
        <div className="grid  gap-4">
          <div className="col-span-2">
            <label className="block text-sm font-medium text-gray-700 mb-1">API Name</label>
            <div className="flex items-center">
              <input
                type="text"
                value={apiName}
                onChange={(e) => setApiName(e.target.value)}
                className="flex-1 border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
                placeholder="Must be unique per project"
              />
            </div>
          </div>
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Version</label>
          <input
            type="text"
            value={version}
            onChange={(e) => setVersion(e.target.value)}
            className="w-full border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
            placeholder="Version prefix for your API"
          />
          <p className="mt-1 text-sm text-gray-500">Version prefix for your API</p>
        </div>

        <div className="flex justify-end">
          <button
            type="submit"
            className="flex items-center space-x-2 bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700"
          >
            Create API
          </button>
        </div>
      </form>
    </div>
  );
};

export default CreateAPI;
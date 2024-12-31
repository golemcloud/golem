import React, { useState } from 'react';
import { Database, Zap } from 'lucide-react';
import WASMUpload from './WASMUpload';
import FileManager from './FileManager';

const CreateComponent = () => {
  const [componentName, setComponentName] = useState('');
  const [type, setType] = useState<'durable' | 'ephemeral'>('durable');

  return (
    <div className="container mx-auto px-4 py-8 max-w-3xl">
      <h1 className="text-2xl font-semibold mb-2">Create a new Component</h1>
      <p className="text-gray-600 mb-8">Components are the building blocks for your project</p>

      <div className="space-y-8">
        {/* Project and Component Name */}
        <div className="grid">
          <div className="col-span-2">
            <label className="block text-sm font-medium text-gray-700 mb-1">Component Name</label>
            <div className="flex items-center">
              <input
                type="text"
                value={componentName}
                onChange={(e) => setComponentName(e.target.value)}
                className="flex-1 border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
                placeholder="Enter component name"
              />
            </div>
          </div>
        </div>

        {/* Type Selection */}
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-3">Type</label>
          <div className="space-y-3">
            <label className="flex items-start space-x-3 p-3 border border-gray-200 rounded cursor-pointer hover:bg-gray-50">
              <input
                type="radio"
                name="type"
                value="durable"
                checked={type === 'durable'}
                onChange={() => setType('durable')}
                className="mt-1"
              />
              <div>
                <div className="flex items-center space-x-2">
                  <Database className="h-5 w-5 text-gray-600" />
                  <span className="font-medium">Durable</span>
                </div>
                <p className="text-sm text-gray-600 mt-1">
                  Workers are persistent and executed with transactional guarantees
                  <br />
                  Ideal for stateful and high-reliability use cases
                </p>
              </div>
            </label>
            <label className="flex items-start space-x-3 p-3 border border-gray-200 rounded cursor-pointer hover:bg-gray-50">
              <input
                type="radio"
                name="type"
                value="ephemeral"
                checked={type === 'ephemeral'}
                onChange={() => setType('ephemeral')}
                className="mt-1"
              />
              <div>
                <div className="flex items-center space-x-2">
                  <Zap className="h-5 w-5 text-gray-600" />
                  <span className="font-medium">Ephemeral</span>
                </div>
                <p className="text-sm text-gray-600 mt-1">
                  Workers are transient and executed normally
                  <br />
                  Ideal for stateless and low-reliability use cases
                </p>
              </div>
            </label>
          </div>
        </div>

        <WASMUpload />
        <FileManager />

        <div className="flex justify-end">
          <button className="flex items-center space-x-2 bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
            <span>Create Component</span>
          </button>
        </div>
      </div>
    </div>
  );
};

export default CreateComponent;
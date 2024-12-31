import React, { useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';

const HTTP_METHODS = ['Get', 'Post', 'Put', 'Patch', 'Delete', 'Head', 'Options', 'Trace', 'Connect'];

const CreateRoute = () => {
  const navigate = useNavigate();
  const { apiName } = useParams();
  const [method, setMethod] = useState('Get');
  const [path, setPath] = useState('');
  const [component, setComponent] = useState('');
  const [version, setVersion] = useState('');
  const [workerName, setWorkerName] = useState('');
  const [response, setResponse] = useState('');

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    // Handle route creation
    navigate(`/apis/${apiName}`);
  };

  return (
    <div className="p-6">
      <div className="flex items-center mb-6">
        <button
          onClick={() => navigate(`/apis/${apiName}`)}
          className="flex items-center text-gray-600 hover:text-gray-900"
        >
          <ArrowLeft className="h-4 w-4 mr-2" />
          <span>New Route</span>
        </button>
      </div>

      <form onSubmit={handleSubmit} className="space-y-8">
        <section>
          <h3 className="text-lg font-medium mb-4">HTTP Endpoint</h3>
          <p className="text-sm text-gray-600 mb-4">
            Each API Route must have a unique Method + Path combination
          </p>
          
          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">Method</label>
              <div className="flex flex-wrap gap-2">
                {HTTP_METHODS.map((m) => (
                  <button
                    key={m}
                    type="button"
                    onClick={() => setMethod(m)}
                    className={`px-3 py-1 rounded ${
                      method === m
                        ? 'bg-gray-100 text-gray-900'
                        : 'text-gray-600 hover:bg-gray-50'
                    }`}
                  >
                    {m}
                  </button>
                ))}
              </div>
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">Path</label>
              <input
                type="text"
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="Define path variables with curly brackets (<VARIABLE_NAME>)"
                className="w-full px-3 py-2 border border-gray-200 rounded-md"
              />
            </div>
          </div>
        </section>

        <section>
          <h3 className="text-lg font-medium mb-4">Worker Binding</h3>
          <p className="text-sm text-gray-600 mb-4">
            Bind this endpoint to a specific worker function
          </p>
          
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">Component</label>
              <input
                type="text"
                value={component}
                onChange={(e) => setComponent(e.target.value)}
                className="w-full px-3 py-2 border border-gray-200 rounded-md"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">Version</label>
              <input
                type="text"
                value={version}
                onChange={(e) => setVersion(e.target.value)}
                className="w-full px-3 py-2 border border-gray-200 rounded-md"
              />
            </div>
          </div>

          <div className="mt-4">
            <label className="block text-sm font-medium text-gray-700 mb-2">Worker Name</label>
            <textarea
              value={workerName}
              onChange={(e) => setWorkerName(e.target.value)}
              placeholder="Interpolate variables into your Worker ID"
              className="w-full px-3 py-2 border border-gray-200 rounded-md h-24"
            />
          </div>
        </section>

        <section>
          <h3 className="text-lg font-medium mb-4">Response</h3>
          <p className="text-sm text-gray-600 mb-4">
            Define the HTTP response for this API Route
          </p>
          
          <textarea
            value={response}
            onChange={(e) => setResponse(e.target.value)}
            className="w-full px-3 py-2 border border-gray-200 rounded-md h-32"
          />
        </section>

        <div className="flex justify-end space-x-3">
          <button
            type="button"
            onClick={() => navigate(`/apis/${apiName}`)}
            className="px-4 py-2 text-gray-600 hover:text-gray-900"
          >
            Clear
          </button>
          <button
            type="submit"
            className="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700"
          >
            Create Route
          </button>
        </div>
      </form>
    </div>
  );
};

export default CreateRoute;
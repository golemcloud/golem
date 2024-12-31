import React from 'react';
import { useParams } from 'react-router-dom';
import { Plus } from 'lucide-react';
import APILeftNav from './APILeftNav';

const APIDetails = () => {
  const { apiName } = useParams();

  return (
    <div className="flex">
      <APILeftNav />
      <div className="flex-1 p-8">
        <div className="flex items-center justify-between mb-8">
          <div>
            <h1 className="text-2xl font-semibold mb-2">{apiName}</h1>
            <div className="flex items-center gap-2">
              <span className="px-2 py-1 bg-gray-100 rounded text-sm">0.1.0</span>
            </div>
          </div>
        </div>

        <div className="space-y-8">
          <section>
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-xl font-semibold">Routes</h2>
              <button className="flex items-center gap-2 text-blue-600 hover:text-blue-700">
                <Plus className="h-5 w-5" />
                <span>Add</span>
              </button>
            </div>
            <div className="bg-white rounded-lg border border-gray-200 p-8 text-center text-gray-500">
              No routes defined for this API version.
            </div>
          </section>

          <section>
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-xl font-semibold">Active Deployments</h2>
              <button className="text-blue-600 hover:text-blue-700">View All</button>
            </div>
            <div className="bg-white rounded-lg border border-gray-200 p-8 text-center text-gray-500">
              No active deployments for this API version.
            </div>
          </section>
        </div>
      </div>
    </div>
  );
};

export default APIDetails;
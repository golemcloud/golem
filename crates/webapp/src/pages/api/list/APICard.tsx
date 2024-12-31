import React from 'react';
import { useNavigate } from 'react-router-dom';

interface APICardProps {
  name: string;
  version: string;
  routes: number;
}

const APICard = ({ name, version, routes }: APICardProps) => {
  const navigate = useNavigate();

  return (
    <div 
      className="bg-white rounded-lg border border-gray-200 p-6 hover:shadow-md transition-shadow cursor-pointer"
      onClick={() => navigate(`/apis/${name}`)}
    >
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-lg font-medium">{name}</h3>
        <div className="flex items-center gap-2">
          <span className="px-2 py-1 bg-gray-100 rounded text-sm">{version}</span>
        </div>
      </div>
      <div className="flex items-center text-sm text-gray-600">
        <span>Routes</span>
        <span className="ml-2">{routes}</span>
      </div>
    </div>
  );
}

export default APICard;
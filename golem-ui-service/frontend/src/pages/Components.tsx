import { Box, Clock, Component as ComponentIcon, Cpu, FileCode, GitBranch, Loader2, Package, Plus, Tag } from 'lucide-react';
import { useRef, useState } from 'react';

import CreateComponentModal from '../components/components/CreateComponentModal';
import { Link } from 'react-router-dom';
import { format } from 'date-fns';
import { useComponents } from '../api/components';

export const Components = () => {
  const [showCreateModal, setShowCreateModal] = useState(false);
  const { data: components, isLoading } = useComponents();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400 flex items-center gap-2">
          <Loader2 className="animate-spin" size={20} />
          <span>Loading components...</span>
        </div>
      </div>
    );
  }

  const getComponentTypeIcon = (type: string) => {
    switch (type.toLowerCase()) {
      case 'service':
        return <Cpu className="text-green-400" size={16} />;
      case 'function':
        return <FileCode className="text-blue-400" size={16} />;
      default:
        return <ComponentIcon className="text-purple-400" size={16} />;
    }
  };

  return (
    <div className="space-y-8">
      <div className="flex justify-between items-center bg-gray-800/50 p-6 rounded-lg">
        <div>
          <h1 className="text-2xl font-bold flex items-center gap-3">
            <Package size={24} className="text-blue-400" />
            Components
          </h1>
          <p className="text-gray-400 mt-1">Manage and deploy your system components</p>
        </div>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded-lg 
                     hover:bg-blue-600 transition-all duration-200 shadow-lg hover:shadow-xl"
        >
          <Plus size={18} />
          Create Component
        </button>
      </div>

      {(!components || components.length === 0) ? (
        <div className="text-center py-12 bg-gray-800 rounded-lg">
          <Box size={48} className="mx-auto text-gray-600 mb-4" />
          <p className="text-gray-400">No components found</p>
          <p className="text-gray-500 text-sm mt-2">Create your first component to get started</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
          {components?.map((component) => (
            <Link
              key={component.versionedComponentId.componentId}
              to={`/components/${component.versionedComponentId.componentId}`}
              className="group block bg-gray-800 rounded-lg p-6 hover:bg-gray-800/80 
                       transition-all duration-200 hover:shadow-xl shadow-lg"
            >
              <div className="flex items-start justify-between">
                <div>
                  <h3 className="font-medium text-lg flex items-center gap-2 group-hover:text-blue-400 transition-colors">
                    {getComponentTypeIcon(component.componentType)}
                    {component.componentName}
                  </h3>
                  <div className="flex items-center gap-2 mt-2 text-sm text-gray-400">
                    <GitBranch size={14} />
                    <span>Version {component.versionedComponentId.version}</span>
                  </div>
                </div>
                <span className="px-2 py-1 rounded-md bg-gray-700/50 text-xs font-medium text-gray-300">
                  {component.componentType}
                </span>
              </div>

              <div className="mt-4 pt-4 border-t border-gray-700/50">
                <div className="flex items-center justify-between text-sm text-gray-400">
                  <div className="flex items-center gap-2">
                    <Clock size={14} />
                    <span>Created</span>
                  </div>
                  <span>{format(new Date(component.createdAt), 'MMM d, yyyy')}</span>
                </div>
                
                <div className="mt-2 flex items-center gap-2">
                  <Tag size={14} className="text-gray-400" />
                  <span className="text-sm text-gray-400">{component.versionedComponentId.componentId}</span>
                </div>
              </div>
            </Link>
          ))}
        </div>
      )}

      <CreateComponentModal
        isOpen={showCreateModal}
        onClose={() => setShowCreateModal(false)}
      />
    </div>
  );
};
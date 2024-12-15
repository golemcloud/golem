import { Clock, Folder, Plus, Upload } from 'lucide-react';
import { useRef, useState } from 'react';

import CreateComponentModal from '../components/components/CreateComponentModal';
import { Link } from 'react-router-dom';
import { format } from 'date-fns';
import { useComponents } from '../api/components';

export const Components = () => {
  const [showCreateModal, setShowCreateModal] = useState(false);
  const { data: components, isLoading } = useComponents();

  if (isLoading) {
    return <div className="text-gray-400">Loading...</div>;
  }

  return (
    <div>
      <div className="flex justify-between items-center mb-6">
        <h1 className="text-2xl font-bold">Components</h1>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-600"
        >
          <Plus size={18} />
          Create Component
        </button>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {components?.map((component) => (
          <Link
            key={component.versionedComponentId.componentId}
            to={`/components/${component.versionedComponentId.componentId}`}
            className="block bg-gray-800 rounded-lg p-4 hover:bg-gray-750 transition-colors"
          >
            <h3 className="font-medium text-lg">{component.componentName}</h3>
            <div className="mt-2 space-y-1">
              <div className="flex items-center text-sm text-gray-400">
                <Clock className="h-4 w-4 mr-2" />
                {format(new Date(component.createdAt), 'MMM d, yyyy')}
              </div>
              <div className="text-sm text-gray-400">
                Version: {component.versionedComponentId.version}
              </div>
              <div className="text-sm text-gray-400">
                Type: {component.componentType}
              </div>
            </div>
          </Link>
        ))}
      </div>

      <CreateComponentModal
        isOpen={showCreateModal}
        onClose={() => setShowCreateModal(false)}
      />
    </div>
  );
};
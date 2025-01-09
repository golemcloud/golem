import {
  Box,
  Clock,
  Component as ComponentIcon,
  Cpu,
  FileCode,
  GitBranch,
  Loader2,
  Package,
  Plus,
  Tag,
} from "lucide-react";
import { useEffect, useState } from "react";

import CreateComponentModal from "../components/components/CreateComponentModal";
import { Link } from "react-router-dom";
import { format } from "date-fns";
import { useComponents } from "../api/components";

export const Components = () => {
  const [showCreateModal, setShowCreateModal] = useState(false);
  const { data: components, isLoading } = useComponents();

  useEffect(() => {
    document.title = `Components - Golem UI`;
  }, []);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground flex items-center gap-2">
          <Loader2 className="animate-spin" size={20} />
          <span>Loading components...</span>
        </div>
      </div>
    );
  }

  const getComponentTypeIcon = (type: string) => {
    switch (type.toLowerCase()) {
      case "service":
        return <Cpu className="text-green-400" size={16} />;
      case "function":
        return <FileCode className="text-primary" size={16} />;
      default:
        return <ComponentIcon className="text-purple-400" size={16} />;
    }
  };

  return (
    <div className="space-y-4 md:space-y-8 px-4 md:px-6">
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4 bg-card/50 p-4 md:p-6 rounded-lg">
        <div>
          <h1 className="text-xl md:text-2xl font-bold flex items-center gap-3">
            <Package size={24} className="text-primary" />
            Components
          </h1>
          <p className="text-sm md:text-base text-muted-foreground mt-1">
            Manage and deploy your system components
          </p>
        </div>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center justify-center gap-2 bg-primary text-white px-4 py-2 rounded-lg 
                   hover:bg-blue-600 transition-all duration-200 shadow-lg hover:shadow-xl w-full sm:w-auto"
        >
          <Plus size={18} />
          Create Component
        </button>
      </div>

      {!components || components.length === 0 ? (
        <div className="text-center py-8 md:py-12 bg-card rounded-lg">
          <Box size={48} className="mx-auto text-gray-600 mb-4" />
          <p className="text-sm md:text-base text-muted-foreground">No components found</p>
          <p className="text-xs md:text-sm text-gray-500 mt-2">
            Create your first component to get started
          </p>
          <button
            onClick={() => setShowCreateModal(true)}
            className="mt-4 text-primary hover:text-primary-accent text-sm"
          >
            Create Component
          </button>
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3 md:gap-6">
          {components?.map((component) => (
            <Link
              key={component.versionedComponentId.componentId}
              to={`/components/${component.versionedComponentId.componentId}/${component.versionedComponentId.version}`}
              className="group block bg-card rounded-lg p-4 md:p-6 hover:bg-card/80 
                       transition-all duration-200 hover:shadow-xl shadow-lg"
            >
              <div className="flex flex-col sm:flex-row sm:items-start sm:justify-between gap-2 sm:gap-4">
                <div className="min-w-0">
                  <h3 className="font-medium text-base md:text-lg flex items-center gap-2 group-hover:text-primary transition-colors">
                    {getComponentTypeIcon(component.componentType)}
                    <span className="truncate">{component.componentName}</span>
                  </h3>
                  <div className="flex items-center gap-2 mt-2 text-xs md:text-sm text-muted-foreground">
                    <GitBranch size={14} />
                    <span>
                      Version {component.versionedComponentId.version}
                    </span>
                  </div>
                </div>
                <span className="px-2 py-1 rounded-md bg-card/50 text-xs font-medium text-gray-300 w-fit">
                  {component.componentType}
                </span>
              </div>

              <div className="mt-4 pt-4 border-t border-gray-700/50">
                <div className="flex items-center justify-between text-xs md:text-sm text-muted-foreground">
                  <div className="flex items-center gap-2">
                    <Clock size={14} />
                    <span>Created</span>
                  </div>
                  <span>
                    {format(new Date(component.createdAt), "MMM d, yyyy")}
                  </span>
                </div>

                <div className="mt-2 flex items-center gap-2 truncate">
                  <Tag size={14} className="text-muted-foreground flex-shrink-0" />
                  <span className="text-xs md:text-sm text-muted-foreground truncate">
                    {component.versionedComponentId.componentId}
                  </span>
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
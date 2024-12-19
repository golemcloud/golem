import {
  Cog,
  Database,
  ExternalLink,
  GitBranch,
  Package,
  Plus,
  Tag,
  Target,
  Trash2,
} from "lucide-react";
import { useDeletePlugin, usePlugins } from "../api/plugins";

import { CreatePluginModal } from "../components/plugins/CreatePluginModal";
import { Link } from "react-router-dom";
import { useState } from "react";

export const PluginsPage = () => {
  const [showCreateModal, setShowCreateModal] = useState(false);
  const { data: plugins, isLoading } = usePlugins();
  const { mutate: deletePlugin } = useDeletePlugin();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400 flex items-center gap-2">
          <Cog className="animate-spin" size={20} />
          <span>Loading plugins...</span>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-8">
      <div className="flex justify-between items-center bg-gray-800/50 p-6 rounded-lg">
        <div>
          <h1 className="text-2xl font-bold flex items-center gap-3">
            <Package size={24} className="text-blue-400" />
            Plugins
          </h1>
          <p className="text-gray-400 mt-1">
            Manage your system plugins and extensions
          </p>
        </div>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded-lg 
                             hover:bg-blue-600 transition-colors duration-200 shadow-lg hover:shadow-xl"
        >
          <Plus size={18} />
          Create Plugin
        </button>
      </div>

      <div className="grid gap-6">
        {plugins?.map((plugin) => (
          <div
            key={`${plugin.name}-${plugin.version}`}
            className="bg-gray-800 rounded-lg p-6 hover:bg-gray-800/80 transition-colors duration-200"
          >
            <div className="flex justify-between items-start">
              <div className="space-y-1">
                <h3 className="text-lg font-medium flex items-center gap-2">
                  {plugin.specs.type === "OplogProcessor" ? (
                    <Database size={18} className="text-purple-400" />
                  ) : (
                    <GitBranch size={18} className="text-green-400" />
                  )}
                  <Link
                    to={`/plugins/${plugin.name}/${plugin.version}`}
                    className="hover:text-blue-400 transition-colors"
                  >
                    {plugin.name}
                  </Link>
                </h3>
                <div className="flex items-center gap-2 text-sm text-gray-400">
                  <Tag size={14} />
                  <span>Version {plugin.version}</span>
                </div>
              </div>
              <button
                onClick={() =>
                  deletePlugin({ name: plugin.name, version: plugin.version })
                }
                className="p-2 text-gray-400 hover:text-red-400 rounded-md hover:bg-gray-700/50
                                         transition-all duration-200"
                title="Delete plugin"
              >
                <Trash2 size={18} />
              </button>
            </div>

            <div className="mt-6 space-y-4">
              <p className="text-gray-300">{plugin.description}</p>

              <div className="flex gap-6 text-gray-400 text-sm">
                <div className="flex items-center gap-2">
                  <Target size={14} />
                  <span>Type: {plugin.specs.type}</span>
                </div>
                <div className="flex items-center gap-2">
                  <Package size={14} />
                  <span>Scope: {plugin.scope.type}</span>
                </div>
              </div>

              {plugin.specs.type === "OplogProcessor" && (
                <div className="bg-gray-700/50 p-4 rounded-lg space-y-2">
                  <div className="flex items-center gap-2 text-sm">
                    <Database size={14} className="text-purple-400" />
                    <span>Component ID: {plugin.specs.componentId}</span>
                  </div>
                  <div className="flex items-center gap-2 text-sm">
                    <Tag size={14} className="text-purple-400" />
                    <span>Version: {plugin.specs.componentVersion}</span>
                  </div>
                </div>
              )}

              {plugin.specs.type === "ComponentTransformer" && (
                <div className="space-y-2 text-sm">
                  <div className="flex items-center gap-2 text-gray-400 hover:text-blue-400 transition-colors">
                    <ExternalLink size={14} />
                    <span>Validate URL: {plugin.specs.validateUrl}</span>
                  </div>
                  <div className="flex items-center gap-2 text-gray-400 hover:text-blue-400 transition-colors">
                    <ExternalLink size={14} />
                    <span>Transform URL: {plugin.specs.transformUrl}</span>
                  </div>
                </div>
              )}
            </div>
          </div>
        ))}

        {(!plugins || plugins.length === 0) && (
          <div className="text-center py-12 bg-gray-800 rounded-lg">
            <Package size={48} className="mx-auto text-gray-600 mb-4" />
            <p className="text-gray-400">No plugins found</p>
            <p className="text-gray-500 text-sm mt-2">
              Create your first plugin to get started
            </p>
          </div>
        )}
      </div>

      <CreatePluginModal
        isOpen={showCreateModal}
        onClose={() => setShowCreateModal(false)}
      />
    </div>
  );
};

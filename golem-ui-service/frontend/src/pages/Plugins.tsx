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
        <div className="text-muted-foreground flex items-center gap-2">
          <Cog className="animate-spin" size={20} />
          <span>Loading plugins...</span>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4 md:space-y-8 px-4 md:px-6">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4 bg-card/50 p-4 md:p-6 rounded-lg">
        <div>
          <h1 className="text-xl md:text-2xl font-bold flex items-center gap-3">
            <Package size={24} className="text-primary" />
            Plugins
          </h1>
          <p className="text-sm md:text-base text-muted-foreground mt-1">
            Manage your system plugins and extensions
          </p>
        </div>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center justify-center gap-2 bg-primary text-white px-4 py-2 rounded-lg 
                   hover:bg-blue-600 transition-colors duration-200 shadow-lg hover:shadow-xl w-full sm:w-auto"
        >
          <Plus size={18} />
          Create Plugin
        </button>
      </div>

      {/* Plugins Grid */}
      <div className="grid gap-3 md:gap-6">
        {plugins?.map((plugin) => (
          <div
            key={`${plugin.name}-${plugin.version}`}
            className="bg-card rounded-lg p-4 md:p-6 hover:bg-card/80 transition-colors duration-200"
          >
            <div className="flex flex-col sm:flex-row sm:justify-between sm:items-start gap-3 sm:gap-4">
              <div className="space-y-1 min-w-0">
                <h3 className="text-base md:text-lg font-medium flex items-center gap-2">
                  {plugin.specs.type === "OplogProcessor" ? (
                    <Database size={18} className="text-purple-400 flex-shrink-0" />
                  ) : (
                    <GitBranch size={18} className="text-green-400 flex-shrink-0" />
                  )}
                  <Link
                    to={`/plugins/${plugin.name}/${plugin.version}`}
                    className="hover:text-primary transition-colors truncate"
                  >
                    {plugin.name}
                  </Link>
                </h3>
                <div className="flex items-center gap-2 text-xs md:text-sm text-muted-foreground">
                  <Tag size={14} className="flex-shrink-0" />
                  <span>Version {plugin.version}</span>
                </div>
              </div>
              <button
                onClick={() => confirm("Delete Plugin?") && deletePlugin({ name: plugin.name, version: plugin.version })}
                className="p-2 text-muted-foreground hover:text-red-400 rounded-md hover:bg-card/50
                         transition-all duration-200"
                title="Delete plugin"
              >
                <Trash2 size={18} />
              </button>
            </div>

            <div className="mt-4 md:mt-6 space-y-3 md:space-y-4">
              <p className="text-gray-300 text-sm md:text-base break-words">{plugin.description}</p>

              <div className="flex flex-col sm:flex-row gap-3 sm:gap-6 text-muted-foreground text-xs md:text-sm">
                <div className="flex items-center gap-2">
                  <Target size={14} className="flex-shrink-0" />
                  <span>Type: {plugin.specs.type}</span>
                </div>
                <div className="flex items-center gap-2">
                  <Package size={14} className="flex-shrink-0" />
                  <span>Scope: {plugin.scope.type}</span>
                </div>
              </div>

              {plugin.specs.type === "OplogProcessor" && (
                <div className="bg-muted/70 p-3 md:p-4 rounded-lg space-y-2">
                  <div className="flex items-center gap-2 text-xs md:text-sm break-all">
                    <Database size={14} className="text-purple-400 flex-shrink-0" />
                    <span>Component ID: {plugin.specs.componentId}</span>
                  </div>
                  <div className="flex items-center gap-2 text-xs md:text-sm">
                    <Tag size={14} className="text-purple-400 flex-shrink-0" />
                    <span>Version: {plugin.specs.componentVersion}</span>
                  </div>
                </div>
              )}

              {plugin.specs.type === "ComponentTransformer" && (
                <div className="space-y-2 text-xs md:text-sm">
                  <div className="flex items-center gap-2 text-muted-foreground hover:text-primary transition-colors break-all">
                    <ExternalLink size={14} className="flex-shrink-0" />
                    <span>Validate URL: {plugin.specs.validateUrl}</span>
                  </div>
                  <div className="flex items-center gap-2 text-muted-foreground hover:text-primary transition-colors break-all">
                    <ExternalLink size={14} className="flex-shrink-0" />
                    <span>Transform URL: {plugin.specs.transformUrl}</span>
                  </div>
                </div>
              )}
            </div>
          </div>
        ))}

        {(!plugins || plugins.length === 0) && (
          <div className="text-center py-8 md:py-12 bg-card rounded-lg">
            <Package size={48} className="mx-auto text-gray-600 mb-4" />
            <p className="text-sm md:text-base text-muted-foreground">No plugins found</p>
            <p className="text-xs md:text-sm text-gray-500 mt-2">
              Create your first plugin to get started
            </p>
            <button
              onClick={() => setShowCreateModal(true)}
              className="mt-4 text-primary hover:text-primary-accent text-sm"
            >
              Create Plugin
            </button>
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
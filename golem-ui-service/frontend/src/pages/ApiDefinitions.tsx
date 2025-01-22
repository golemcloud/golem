import { Globe, Plus, Trash2 } from "lucide-react";
import {
  useApiDefinitions,
  useDeleteApiDefinition,
} from "../api/api-definitions";

import { ApiDefinition } from "../types/api";
import { ApiDefinitionModal } from "../components/api/ApiDefinitionModal";
import { Link } from "react-router-dom";
import { useState } from "react";

const ApiDefinitionCard = ({ apiDef }: { apiDef: ApiDefinition }) => {
  const { mutate: deleteDefinition } = useDeleteApiDefinition();

  return (
    <div className="bg-card rounded-lg p-3 md:p-4 hover:bg-gray-750">
      <div className="flex flex-col sm:flex-row sm:justify-between gap-3 sm:gap-0">
        <Link
          to={`/apis/definitions/${apiDef.id}/${apiDef.version}`}
          className="flex-1 min-w-0"
        >
          <h3 className="font-medium flex items-center gap-2 text-sm md:text-base">
            <Globe className="h-4 w-4 flex-shrink-0" />
            <span className="truncate">{apiDef.id}</span>
          </h3>
          <div className="mt-1 text-xs md:text-sm text-muted-foreground flex flex-wrap gap-2">
            <span>Version {apiDef.version}</span>
            <span className="hidden sm:inline">•</span>
            <span>{apiDef.routes.length} routes</span>
            {apiDef.draft && (
              <>
                <span className="hidden sm:inline">•</span>
                <span className="text-yellow-500">Draft</span>
              </>
            )}
          </div>
        </Link>

        <div className="flex items-start gap-2 sm:ml-4">
          <button
            onClick={() => {
              if (
                window.confirm(
                  "Are you sure you want to delete this API definition?",
                )
              ) {
                deleteDefinition({ id: apiDef.id, version: apiDef.version });
              }
            }}
            className="p-1.5 text-red-400 hover:text-red-300 rounded-md hover:bg-card/50"
            aria-label="Delete API definition"
          >
            <Trash2 size={16} />
          </button>
        </div>
      </div>
    </div>
  );
};

export const ApiDefinitionsPage = () => {
  const [showCreateModal, setShowCreateModal] = useState(false);
  const { data: apiDefinitions, isLoading } = useApiDefinitions();

  document.title = `API Definitions - Golem UI`;

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  return (
    <div className="space-y-4 md:space-y-6 px-4 md:px-6">
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4">
        <h1 className="text-xl md:text-2xl font-bold">API Definitions</h1>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center justify-center gap-2 bg-primary text-white px-4 py-2 rounded hover:bg-blue-600 w-full sm:w-auto"
        >
          <Plus size={18} />
          <span>Create API Definition</span>
        </button>
      </div>

      <div className="grid gap-3 md:gap-4">
        {apiDefinitions?.map((apiDef) => (
          <ApiDefinitionCard
            key={`${apiDef.id}-${apiDef.version}`}
            apiDef={apiDef}
          />
        ))}

        {(!apiDefinitions || apiDefinitions.length === 0) && (
          <div className="text-center py-6 md:py-8 bg-card rounded-lg">
            <p className="text-sm md:text-base text-muted-foreground">
              No API definitions found
            </p>
            <button
              onClick={() => setShowCreateModal(true)}
              className="text-primary hover:text-primary-accent mt-2 text-sm"
            >
              Create your first API definition
            </button>
          </div>
        )}
      </div>

      {/* Create Modal */}
      {showCreateModal && (
        <ApiDefinitionModal
          isOpen={showCreateModal}
          onClose={() => setShowCreateModal(false)}
          onApiDefinitionCreated={() => setShowCreateModal(false)}
        />
      )}
    </div>
  );
};

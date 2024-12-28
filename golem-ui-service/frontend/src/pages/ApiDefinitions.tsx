import { Globe, Plus, Trash2 } from "lucide-react";
import {
  useApiDefinitions,
  useDeleteApiDefinition,
} from "../api/api-definitions";

import { ApiDefinition } from "../types/api";
import { ApiDefinitionModal } from "../components/api/ApiDefinitionModal";
import { Link } from "react-router-dom";
import toast from "react-hot-toast";
import { useState } from "react";

const ApiDefinitionCard = ({ apiDef }: { apiDef: ApiDefinition }) => {
  const { mutate: deleteDefinition } = useDeleteApiDefinition({
    onSuccess: () => toast.success("API definition deleted"),
    onError: () => toast.error("Failed to delete API definition"),
  });

  return (
    <div className="bg-card rounded-lg p-4 hover:bg-gray-750">
      <div className="flex justify-between">
        <Link
          to={`/api/definitions/${apiDef.id}/${apiDef.version}`}
          className="flex-1"
        >
          <h3 className="font-medium flex items-center gap-2">
            <Globe className="h-4 w-4" />
            {apiDef.id}
          </h3>
          <div className="mt-1 text-sm text-muted-foreground">
            <span>Version {apiDef.version}</span>
            <span className="mx-2">•</span>
            <span>{apiDef.routes.length} routes</span>
            {apiDef.draft && (
              <>
                <span className="mx-2">•</span>
                <span className="text-yellow-500">Draft</span>
              </>
            )}
          </div>
        </Link>

        <div className="flex items-start gap-2">
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

  if (isLoading) {
    return <div className="text-muted-foreground">Loading...</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-2xl font-bold">API Definitions</h1>
        <button
          onClick={() => setShowCreateModal(true)}
          className="flex items-center gap-2 bg-primary text-white px-4 py-2 rounded hover:bg-blue-600"
        >
          <Plus size={18} />
          Create API Definition
        </button>
      </div>

      <div className="grid gap-4">
        {apiDefinitions?.map((apiDef) => (
          <ApiDefinitionCard
            key={`${apiDef.id}-${apiDef.version}`}
            apiDef={apiDef}
          />
        ))}

        {(!apiDefinitions || apiDefinitions.length === 0) && (
          <div className="text-center py-8 bg-card rounded-lg">
            <p className="text-muted-foreground">No API definitions found</p>
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

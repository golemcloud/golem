import {
  ArrowLeft,
  Box,
  Code2,
  Globe,
  Plus,
  Route as RouteIcon,
  Share2,
  Trash2,
  Upload,
} from "lucide-react";
import { Link, useParams } from "react-router-dom";
import {
  useApiDefinition,
  useApiDeployments,
  useDeleteDeployment,
  useUpdateApiDefinition,
} from "../api/api-definitions";

import DeployModal from "../components/api/DeployModal";
import RouteModal from "../components/api/ApiRoutesModal";
import toast from "react-hot-toast";
import { useState } from "react";

export interface Route {
  method: string;
  path: string;
  binding: {
    componentId: {
      componentId: string;
      version: number;
    };
    workerName: string;
    response?: string;
    bindingType: "default";
  };
}

// interface ApiDefinition {
//     id: string;
//     version: string;
//     routes: Route[];
//     draft: boolean;
// }

export const ApiDefinitionView = () => {
  const { id, version } = useParams<{ id: string; version: string }>();
  const [showRouteModal, setShowRouteModal] = useState(false);
  const [showDeployModal, setShowDeployModal] = useState(false);
  const [editingRoute, setEditingRoute] = useState<
    (Route & { index: number }) | null
  >(null);

  const { data: apiDefinition, isLoading: isLoadingDefinition } =
    useApiDefinition(id!, version!);

  const { data: deployments, isLoading: isLoadingDeployments } =
    useApiDeployments(id!);
  const deleteDeployment = useDeleteDeployment();
  const updateDefinition = useUpdateApiDefinition();
  // const deleteDeployment = useDeleteDeployment();

  const handleAddRoute = (route: Route) => {
    if (!apiDefinition) return;

    const updatedDefinition = {
      ...apiDefinition,
      routes: [...apiDefinition.routes, route],
    };

    updateDefinition.mutate(
      { id: id!, version: version!, definition: updatedDefinition },
      {
        onSuccess: () => toast.success("Route added successfully"),
        onError: () => toast.error("Failed to add route"),
      },
    );
  };

  const handleDeleteRoute = (index: number) => {
    if (!apiDefinition) return;

    const updatedDefinition = {
      ...apiDefinition,
      routes: apiDefinition.routes.filter((_, i) => i !== index),
    };

    updateDefinition.mutate(
      { id: id!, version: version!, definition: updatedDefinition },
      {
        onSuccess: () => toast.success("Route deleted successfully"),
        onError: () => toast.error("Failed to delete route"),
      },
    );
  };

  const handleEditRoute = (route: Route, index: number) => {
    setEditingRoute({ ...route, index });
    setShowRouteModal(true);
  };

  const handleUpdateRoute = (updatedRoute: Route) => {
    if (!apiDefinition || editingRoute === null) return;

    const updatedRoutes = [...apiDefinition.routes];
    updatedRoutes[editingRoute.index] = updatedRoute;

    const updatedDefinition = {
      ...apiDefinition,
      routes: updatedRoutes,
    };

    updateDefinition.mutate(
      { id: id!, version: version!, definition: updatedDefinition },
      {
        onSuccess: () => {
          toast.success("Route updated successfully");
          setEditingRoute(null);
        },
        onError: () => toast.error("Failed to update route"),
      },
    );
  };

  const handleDeleteDeployment = async (site: string) => {
    try {
      await deleteDeployment.mutateAsync(site);
      toast.success("Deployment deleted successfully");
    } catch (error) {
      toast.error("Failed to delete deployment");
      console.log(error);
    }
  };

  if (isLoadingDefinition || isLoadingDeployments) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400">Loading...</div>
      </div>
    );
  }

  if (!apiDefinition) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400">API definition not found</div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <Link
            to="/api"
            className="p-2 text-gray-400 hover:text-gray-300 rounded-md hover:bg-gray-800"
          >
            <ArrowLeft size={20} />
          </Link>
          <div>
            <h1 className="text-2xl font-bold flex items-center gap-2">
              <Globe className="h-6 w-6 text-blue-400" />
              {apiDefinition.id}
              {apiDefinition.draft && (
                <span className="text-sm bg-yellow-500/10 text-yellow-500 px-2 py-0.5 rounded">
                  Draft
                </span>
              )}
            </h1>
            <p className="text-gray-400">Version {apiDefinition.version}</p>
          </div>
        </div>

        <div className="flex gap-2">
          <button
            onClick={() => setShowDeployModal(true)}
            className="flex items-center gap-2 px-4 py-2 bg-green-500 text-white rounded hover:bg-green-600"
          >
            <Upload size={18} />
            Deploy
          </button>
          <button
            onClick={() => setShowRouteModal(true)}
            className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-600"
          >
            <Plus size={18} />
            Add Route
          </button>
        </div>
      </div>

      <div className="grid grid-cols-3 gap-6">
        {/* Routes List */}
        <div className="col-span-2 bg-gray-800 rounded-lg p-6">
          <div className="flex items-center justify-between mb-4">
            <h2 className="text-lg font-semibold flex items-center gap-2">
              <RouteIcon className="h-5 w-5 text-blue-400" />
              Routes
              <span className="text-sm text-gray-400">
                ({apiDefinition.routes.length})
              </span>
            </h2>
            {apiDefinition.routes.length > 0 && (
              <button
                onClick={() => {
                  const text = apiDefinition.routes
                    .map((r) => `${r.method} ${r.path}`)
                    .join("\n");
                  navigator.clipboard.writeText(text);
                  toast.success("Routes copied to clipboard");
                }}
                className="text-sm text-gray-400 hover:text-gray-300 flex items-center gap-1"
              >
                <Share2 size={14} />
                Copy All
              </button>
            )}
          </div>

          <div className="space-y-3">
            {apiDefinition.routes.map((route, index) => (
              <div
                key={index}
                className="bg-gray-700 rounded-lg p-4 hover:bg-gray-650 transition-colors"
              >
                <div className="flex items-start justify-between">
                  <div className="space-y-2">
                    <div className="flex items-center gap-2">
                      <span
                        className={`
                        px-2 py-0.5 rounded text-sm font-medium
                        ${
                          route.method === "GET"
                            ? "bg-green-500/10 text-green-500"
                            : route.method === "POST"
                              ? "bg-blue-500/10 text-blue-500"
                              : route.method === "PUT"
                                ? "bg-yellow-500/10 text-yellow-500"
                                : route.method === "DELETE"
                                  ? "bg-red-500/10 text-red-500"
                                  : route.method === "PATCH"
                                    ? "bg-purple-500/10 text-purple-500"
                                    : "bg-gray-500/10 text-gray-500"
                        }
                      `}
                      >
                        {route.method}
                      </span>
                      <span className="font-mono">{route.path}</span>
                    </div>

                    <div className="text-sm text-gray-400 space-y-1">
                      <div className="flex items-center gap-2">
                        <Code2 className="h-4 w-4" />
                        Component: {route.binding.componentId.componentId} (v
                        {route.binding.componentId.version})
                      </div>
                      <div className="flex items-center gap-2">
                        <Box className="h-4 w-4" />
                        Worker: {route.binding.workerName}
                      </div>
                      {route.binding.response && (
                        <div className="flex items-center gap-2 text-gray-500">
                          Response Type: {route.binding.response}
                        </div>
                      )}
                    </div>
                  </div>

                  <div className="flex gap-2">
                    <button
                      onClick={() => handleEditRoute(route, index)}
                      className="p-1.5 text-blue-400 hover:text-blue-300 rounded-md hover:bg-gray-600"
                    >
                      <Code2 size={16} />
                    </button>
                    <button
                      onClick={() => {
                        if (
                          window.confirm(
                            "Are you sure you want to delete this route?",
                          )
                        ) {
                          handleDeleteRoute(index);
                        }
                      }}
                      className="p-1.5 text-red-400 hover:text-red-300 rounded-md hover:bg-gray-600"
                    >
                      <Trash2 size={16} />
                    </button>
                  </div>
                </div>
              </div>
            ))}

            {apiDefinition.routes.length === 0 && (
              <div className="text-center py-8 text-gray-400">
                <RouteIcon className="h-8 w-8 mx-auto mb-2 opacity-50" />
                <p>No routes defined yet</p>
                <button
                  onClick={() => setShowRouteModal(true)}
                  className="text-blue-400 hover:text-blue-300 mt-2"
                >
                  Add your first route
                </button>
              </div>
            )}
          </div>
        </div>

        {/* Deployments Panel */}
        <div className="col-span-1 bg-gray-800 rounded-lg p-6">
          <h2 className="text-lg font-semibold mb-4 flex items-center gap-2">
            <Upload className="h-5 w-5 text-green-400" />
            Deployments
          </h2>

          <div className="space-y-3">
            {deployments?.map((deployment) => (
              <div
                key={`${deployment.site.host}-${deployment.site.subdomain}`}
                className="bg-gray-700 rounded-lg p-3"
              >
                <div className="flex items-center justify-between">
                  <div>
                    <div className="flex items-center gap-2">
                      <Globe size={16} className="text-gray-400" />
                      <span>{deployment.site.host}</span>
                    </div>
                    {deployment.site.subdomain && (
                      <p className="text-sm text-gray-400 mt-1">
                        Subdomain: {deployment.site.subdomain}
                      </p>
                    )}
                  </div>
                  <button
                    onClick={() => {
                      if (
                        window.confirm(
                          "Are you sure you want to delete this deployment?",
                        )
                      ) {
                        handleDeleteDeployment(
                          `${deployment.site.subdomain}.${deployment.site.host}`,
                        );
                      }
                    }}
                    className="p-1.5 text-red-400 hover:text-red-300 rounded-md hover:bg-gray-600"
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              </div>
            ))}

            {(!deployments || deployments.length === 0) && (
              <div className="text-center py-4 text-gray-400">
                <p>No active deployments</p>
                <button
                  onClick={() => setShowDeployModal(true)}
                  className="text-green-400 hover:text-green-300 mt-2"
                >
                  Deploy this API
                </button>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Modals */}
      <RouteModal
        isOpen={showRouteModal}
        onClose={() => {
          setShowRouteModal(false);
          setEditingRoute(null);
        }}
        onSave={editingRoute ? handleUpdateRoute : handleAddRoute}
        existingRoute={editingRoute}
      />

      <DeployModal
        isOpen={showDeployModal}
        onClose={() => setShowDeployModal(false)}
        apiDefinition={apiDefinition}
      />
    </div>
  );
};

export default ApiDefinitionView;

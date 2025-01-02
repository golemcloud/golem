import {
  ArrowLeft,
  Box,
  Code2,
  Globe,
  Menu,
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
import { useEffect, useState } from "react";

import DeployModal from "../components/api/DeployModal";
import RouteModal from "../components/api/ApiRoutesModal";
import toast from "react-hot-toast";

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

export const ApiDefinitionView = () => {
  const { id, version } = useParams<{ id: string; version: string }>();
  const [showRouteModal, setShowRouteModal] = useState(false);
  const [showDeployModal, setShowDeployModal] = useState(false);
  const [showMobileMenu, setShowMobileMenu] = useState(false);
  const [editingRoute, setEditingRoute] = useState<(Route & { index: number }) | null>(null);

  const { data: apiDefinition, isLoading: isLoadingDefinition } = useApiDefinition(id!, version!);
  const { data: deployments, isLoading: isLoadingDeployments } = useApiDeployments(id!);
  const deleteDeployment = useDeleteDeployment();
  const updateDefinition = useUpdateApiDefinition();

  useEffect(() => {
    if (apiDefinition) {
      document.title = `${apiDefinition.id} v${apiDefinition.version} - API Definition`;
    }
  }, [apiDefinition]);

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
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  if (!apiDefinition) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">API definition not found</div>
      </div>
    );
  }

  return (
    <div className="space-y-4 md:space-y-6 px-4 md:px-6">
      {/* Header */}
      <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-4">
        <div className="flex items-center gap-4">
          <Link
            to="/apis"
            className="p-2 text-muted-foreground hover:text-gray-300 rounded-md hover:bg-card"
          >
            <ArrowLeft size={20} />
          </Link>
          <div className="min-w-0">
            <h1 className="text-xl md:text-2xl font-bold flex items-center gap-2 flex-wrap">
              <Globe className="h-6 w-6 text-primary flex-shrink-0" />
              <span className="truncate">{apiDefinition.id}</span>
              {apiDefinition.draft && (
                <span className="text-sm bg-yellow-500/10 text-yellow-500 px-2 py-0.5 rounded">
                  Draft
                </span>
              )}
            </h1>
            <p className="text-sm text-muted-foreground">Version {apiDefinition.version}</p>
          </div>
        </div>

        {/* Mobile Menu Button */}
        <div className="md:hidden">
          <button
            onClick={() => setShowMobileMenu(!showMobileMenu)}
            className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60"
          >
            <Menu size={24} />
          </button>
        </div>

        {/* Action Buttons */}
        <div className={`flex flex-col sm:flex-row gap-2 ${showMobileMenu ? 'block' : 'hidden md:flex'}`}>
          <button
            onClick={() => setShowDeployModal(true)}
            className="flex items-center justify-center gap-2 px-4 py-2 bg-green-500 text-white rounded hover:bg-green-600"
          >
            <Upload size={18} />
            <span>Deploy</span>
          </button>
          <button
            onClick={() => setShowRouteModal(true)}
            className="flex items-center justify-center gap-2 bg-primary text-white px-4 py-2 rounded hover:bg-blue-600"
          >
            <Plus size={18} />
            <span>Add Route</span>
          </button>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 md:gap-6">
        {/* Routes List */}
        <div className="md:col-span-2 bg-card rounded-lg p-4 md:p-6">
          <div className="flex items-center justify-between mb-4">
            <h2 className="text-base md:text-lg font-semibold flex items-center gap-2">
              <RouteIcon className="h-5 w-5 text-primary" />
              Routes
              <span className="text-sm text-muted-foreground">
                ({apiDefinition.routes.length})
              </span>
            </h2>
            {apiDefinition.routes.length > 0 && (
              <button
                onClick={() => {
                  const text = apiDefinition.routes.map((r) => `${r.method} ${r.path}`).join("\n");
                  navigator.clipboard.writeText(text);
                  toast.success("Routes copied to clipboard");
                }}
                className="text-sm text-muted-foreground hover:text-gray-300 flex items-center gap-1"
              >
                <Share2 size={14} />
                <span className="hidden sm:inline">Copy All</span>
              </button>
            )}
          </div>

          <div className="space-y-3">
            {apiDefinition.routes.map((route, index) => (
              <div
                key={index}
                className="bg-card/80 hover:bg-card/60 border border-border/10 hover:border-border/20 rounded-lg p-3 md:p-4 transition-colors"
              >
                <div className="flex flex-col sm:flex-row sm:items-start sm:justify-between gap-3">
                  <div className="space-y-2 min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span
                        className={`
                        px-2 py-0.5 rounded text-xs md:text-sm font-medium
                        ${route.method === "GET"
                            ? "bg-green-500/10 text-green-500"
                            : route.method === "POST"
                              ? "bg-primary/10 text-blue-500"
                              : route.method === "PUT"
                                ? "bg-yellow-500/10 text-yellow-500"
                                : route.method === "DELETE"
                                  ? "bg-red-500/10 text-red-500"
                                  : route.method === "PATCH"
                                    ? "bg-purple-500/10 text-purple-500"
                                    : "bg-gray-500/10 text-gray-500"
                          }`}
                      >
                        {route.method}
                      </span>
                      <span className="font-mono text-sm break-all">{route.path}</span>
                    </div>

                    <div className="text-xs md:text-sm text-muted-foreground space-y-1">
                      <div className="flex items-center gap-2">
                        <Code2 className="h-4 w-4 flex-shrink-0" />
                        <span className="break-all">
                          Component: {route.binding.componentId.componentId} (v{route.binding.componentId.version})
                        </span>
                      </div>
                      <div className="flex items-center gap-2">
                        <Box className="h-4 w-4 flex-shrink-0" />
                        <span className="break-all">Worker: {route.binding.workerName}</span>
                      </div>
                      {route.binding.response && (
                        <div className="flex items-center gap-2 text-gray-500">
                          <span className="break-all">Response Type: {route.binding.response}</span>
                        </div>
                      )}
                    </div>
                  </div>

                  <div className="flex gap-2 sm:flex-shrink-0">
                    <button
                      onClick={() => handleEditRoute(route, index)}
                      className="p-1.5 text-primary hover:text-primary-accent rounded-md hover:bg-gray-600"
                    >
                      <Code2 size={16} />
                    </button>
                    <button
                      onClick={() => {
                        if (window.confirm("Are you sure you want to delete this route?")) {
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
              <div className="text-center py-6 md:py-8 text-muted-foreground">
                <RouteIcon className="h-8 w-8 mx-auto mb-2 opacity-50" />
                <p className="text-sm md:text-base">No routes defined yet</p>
                <button
                  onClick={() => setShowRouteModal(true)}
                  className="text-primary hover:text-primary-accent mt-2 text-sm"
                >
                  Add your first route
                </button>
              </div>
            )}
          </div>
        </div>

        {/* Deployments Panel */}
        <div className="bg-card rounded-lg p-4 md:p-6">
          <h2 className="text-base md:text-lg font-semibold mb-4 flex items-center gap-2">
            <Upload className="h-5 w-5 text-green-400" />
            Deployments
          </h2>

          <div className="space-y-3">
            {deployments?.map((deployment) => (
              <div
                key={`${deployment.site.host}-${deployment.site.subdomain}`}
                className="bg-card/80 rounded-lg p-3"
              >
                <div className="flex items-center justify-between">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <Globe size={16} className="text-muted-foreground flex-shrink-0" />
                      <span className="truncate">{deployment.site.host}</span>
                    </div>
                    {deployment.site.subdomain && (
                      <p className="text-xs md:text-sm text-muted-foreground mt-1 truncate">
                        Subdomain: {deployment.site.subdomain}
                      </p>
                    )}
                  </div>
                  <button
                    onClick={() => {
                      if (window.confirm("Are you sure you want to delete this deployment?")) {
                        handleDeleteDeployment(`${deployment.site.subdomain}.${deployment.site.host}`);
                      }
                    }}
                    className="p-1.5 text-red-400 hover:text-red-300 rounded-md hover:bg-gray-600 flex-shrink-0"
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              </div>
            ))}

            {(!deployments || deployments.length === 0) && (
              <div className="text-center py-4 text-muted-foreground">
                <p className="text-sm md:text-base">No active deployments</p>
                <button
                  onClick={() => setShowDeployModal(true)}
                  className="text-green-400 hover:text-green-300 mt-2 text-sm"
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
import { useEffect, useState } from "react";
import { ChevronRight, Copy, Layers, Plus, Trash } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Api } from "@/types/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { API } from "@/service";
import { Deployment } from "@/types/deployments";
import ErrorBoundary from "@/components/errorBoundary";
import { removeDuplicateApis } from "@/lib/utils";

const RoutesCard = ({
  apiId,
  version,
  apiList,
  host,
}: {
  apiId: string;
  version: string;
  host: string;
  apiList: Api[];
}) => {
  const routes = apiList.find(
    (api) => api.id === apiId && api.version === version
  )?.routes;
  const navigate = useNavigate();
  const [hoveredPath, setHoveredPath] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

  const copyToClipboard = (endpoint: { path: string; method: string }) => {
    const fullUrl = `${host}${endpoint.path}`;
    const curlCommand = `curl -X ${endpoint.method} ${fullUrl} -H "Content-Type: application/json" -d '{}'`;
    navigator.clipboard
      .writeText(curlCommand)
      .then(() => {
        setCopiedPath(endpoint.path);
        setTimeout(() => setCopiedPath(null), 2000);
      })
      .catch((err) => console.error("Failed to copy:", err));
  };

  return (
    routes && (
      <div className="space-y-2">
        {routes.map((endpoint, index) => (
          <div
            key={index}
            className="flex items-center space-x-2 cursor-pointer group"
            onMouseEnter={() => setHoveredPath(endpoint.path)}
            onMouseLeave={() => setHoveredPath(null)}
            onClick={() =>
              navigate(
                `/apis/${apiId}/version/${version}/routes?path=${endpoint.path}&method=${endpoint.method}`
              )
            }
          >
            {/* Left Side: API Method & Path */}
            <div className="flex items-center space-x-2">
              <span className="px-2 py-0.5 text-xs font-medium rounded bg-emerald-100 dark:bg-emerald-900 text-emerald-700 dark:text-emerald-200">
                {endpoint.method}
              </span>
              <code className="text-sm font-mono">{endpoint.path}</code>
            </div>

            {/* Right Side: Copy Button (Shown on Hover) */}
            {hoveredPath === endpoint.path && (
              <button
                onClick={(e) => {
                  e.stopPropagation(); // Prevent navigation when clicking copy
                  copyToClipboard(endpoint);
                }}
                className="flex items-center space-x-1 text-muted-foreground hover:text-primary transition"
              >
                <Copy className="w-4 h-4" />
                <span className="text-xs">
                  {copiedPath === endpoint.path ? "✅ Copied!" : "Copy Curl"}
                </span>
              </button>
            )}
          </div>
        ))}
      </div>
    )
  );
};

export default function Deployments() {
  const navigate = useNavigate();
  const [expandedDeployment, setExpandedDeployment] = useState<string[]>([]);
  const [apiList, setApiList] = useState<Api[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [isDialogOpen, setIsDialogOpen] = useState(false);
  const [selectedDeploymentHost, setSelectedDeploymentHost] = useState<
    string | null
  >(null);

  useEffect(() => {
    const fetchDeployments = async () => {
      try {
        const response = await API.getApiList();
        setApiList(response);

        const uniqueApis = removeDuplicateApis(response);
        const allDeployments = await Promise.all(
          uniqueApis.map((api) => API.getDeploymentApi(api.id))
        );

        setDeployments(allDeployments.flat().filter(Boolean));
      } catch (error) {
        console.error("Error fetching deployments:", error);
      }
    };

    fetchDeployments();
  }, []);

  const handleDelete = async () => {
    if (!selectedDeploymentHost) return;

    try {
      await API.deleteDeployment(selectedDeploymentHost);
      setDeployments((prev) =>
        prev.filter((d) => d.site.host !== selectedDeploymentHost)
      );
    } catch (error) {
      console.error("Error deleting deployment:", error);
    } finally {
      setIsDialogOpen(false);
      setSelectedDeploymentHost(null);
    }
  };

  const toggleExpanded = (host: string, apiId: string, version: string) => {
    setExpandedDeployment((prev) =>
      prev.includes(`${host}.${apiId}.${version}`)
        ? prev.filter((item) => item !== `${host}.${apiId}.${version}`)
        : [...prev, `${host}.${apiId}.${version}`]
    );
  };

  return (
    <ErrorBoundary>
      <div className="p-6 mx-auto max-w-7xl">
        <div className="flex items-center justify-between mb-6">
          <h1 className="text-xl font-semibold">API Deployments</h1>
          <Button size="sm" onClick={() => navigate("/deployments/create")}>
            <Plus className="w-4 h-4 mr-2" />
            New
          </Button>
        </div>

        <div className="space-y-4">
          {deployments.length > 0 ? (
            <div className="grid gap-6 overflow-scroll max-h-[80vh]">
              {deployments.map((deployment) => (
                <Card key={deployment.site.host} className="p-6">
                  <div className="space-y-6">
                    <div className="flex items-center justify-between">
                      <h2 className="text-base font-medium">
                        {deployment.site.host}
                      </h2>

                      <Dialog
                        open={isDialogOpen}
                        onOpenChange={setIsDialogOpen}
                      >
                        <DialogTrigger asChild>
                          <Button
                            variant="destructive"
                            size="icon"
                            onClick={(e) => {
                              e.stopPropagation();
                              setSelectedDeploymentHost(deployment.site.host);
                              setIsDialogOpen(true);
                            }}
                          >
                            <Trash className="h-4 w-4" />
                          </Button>
                        </DialogTrigger>
                        <DialogContent>
                          <DialogHeader>
                            <DialogTitle>Delete Deployment</DialogTitle>
                            <DialogDescription>
                              Are you sure you want to delete{" "}
                              <strong>{selectedDeploymentHost}</strong>? This
                              action cannot be undone.
                            </DialogDescription>
                          </DialogHeader>
                          <DialogFooter>
                            <Button
                              variant="destructive"
                              onClick={handleDelete}
                            >
                              Confirm Delete
                            </Button>
                          </DialogFooter>
                        </DialogContent>
                      </Dialog>
                    </div>

                    <div className="space-y-2">
                      {deployment.apiDefinitions.map((api) => (
                        <div key={api.id} className="grid space-y-2">
                          <div className="flex justify-between">
                            <div className="flex items-center gap-4">
                              <span
                                className="relative rounded bg-muted p-1 font-mono text-sm cursor-pointer"
                                onClick={() =>
                                  navigate(
                                    `/apis/${api.id}/version/${api.version}`
                                  )
                                }
                              >
                                {api.id} (v{api.version})
                              </span>

                              {(apiList.find(
                                (a) =>
                                  a.id === api.id && a.version === api.version
                              )?.routes?.length || 0) > 0 && (
                                <button
                                  onClick={() =>
                                    toggleExpanded(
                                      deployment.site.host,
                                      api.id,
                                      api.version
                                    )
                                  }
                                  className="p-1 hover:bg-accent rounded-md"
                                >
                                  <ChevronRight
                                    className={`w-4 h-4 text-muted-foreground transition-transform ${
                                      expandedDeployment.includes(
                                        `${deployment.site.host}.${api.id}.${api.version}`
                                      )
                                        ? "rotate-90"
                                        : ""
                                    }`}
                                  />
                                </button>
                              )}
                            </div>
                          </div>

                          {expandedDeployment.includes(
                            `${deployment.site.host}.${api.id}.${api.version}`
                          ) && (
                            <RoutesCard
                              apiId={api.id}
                              version={api.version}
                              apiList={apiList}
                              host={deployment.site.host}
                            />
                          )}
                        </div>
                      ))}
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          ) : (
            <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-muted rounded-lg">
              <Layers className="h-12 w-12 text-muted-foreground mb-4" />
              <h3 className="text-lg font-medium mb-2">No Deployments</h3>
              <p className="text-muted-foreground mb-4">
                Create your first deployment to get started.
              </p>
            </div>
          )}
        </div>
      </div>
    </ErrorBoundary>
  );
}

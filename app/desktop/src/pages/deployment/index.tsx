import ErrorBoundary from "@/components/errorBoundary";
import { HTTP_METHOD_COLOR } from "@/components/nav-route";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { cn, removeDuplicateApis } from "@/lib/utils";
import { API } from "@/service";
import { Api } from "@/types/api";
import { Deployment } from "@/types/deployments";
import { ChevronRight, Copy, Layers, Plus, Trash } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";

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
    api => api.id === apiId && api.version === version,
  )?.routes;
  const navigate = useNavigate();
  const [hoveredPath, setHoveredPath] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

  const copyToClipboard = (endpoint: { path: string; method: string }) => {
    const fullUrl = `${host}${endpoint.path}`;
    const curlCommand = `curl --location ${endpoint.method} http://${fullUrl} --header "Content-Type: application/json" -d '{}'`;
    navigator.clipboard
      .writeText(curlCommand)
      .then(() => {
        setCopiedPath(endpoint.path);
        setTimeout(() => setCopiedPath(null), 2000);
      })
      .catch(err => console.error("Failed to copy:", err));
  };

  return (
    routes && (
      <Card className="bg-transparent">
        <CardContent className="space-y-2 p-4">
          {routes.map((endpoint, index) => (
            <div
              key={index}
              className="flex items-center justify-between p-2 rounded-lg cursor-pointer group hover:bg-muted transition"
              onMouseEnter={() => setHoveredPath(endpoint.path)}
              onMouseLeave={() => setHoveredPath(null)}
              onClick={() =>
                navigate(
                  `/apis/${apiId}/version/${version}/routes?path=${endpoint.path}&method=${endpoint.method}`,
                )
              }
            >
              <div className="flex flex-row gap-3">
                <Badge
                  variant="secondary"
                  className={cn(
                    HTTP_METHOD_COLOR[
                      endpoint.method as keyof typeof HTTP_METHOD_COLOR
                    ],
                    "w-16 text-center justify-center",
                  )}
                >
                  {endpoint.method}
                </Badge>
                <code className="text-sm font-mono text-foreground">
                  {endpoint.path}
                </code>
              </div>
              {hoveredPath === endpoint.path && (
                <button
                  onClick={e => {
                    e.stopPropagation();
                    copyToClipboard(endpoint);
                  }}
                  className="flex items-center gap-1 text-muted-foreground hover:text-primary transition"
                >
                  <Copy className="w-4 h-4" />
                  <span className="text-xs">
                    {copiedPath === endpoint.path ? "✅ Copied!" : "Copy Curl"}
                  </span>
                </button>
              )}
            </div>
          ))}
        </CardContent>
      </Card>
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
          uniqueApis.map(api => API.getDeploymentApi(api.id)),
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
      setDeployments(prev =>
        prev.filter(d => d.site.host !== selectedDeploymentHost),
      );
    } catch (error) {
      console.error("Error deleting deployment:", error);
    } finally {
      setIsDialogOpen(false);
      setSelectedDeploymentHost(null);
    }
  };

  const toggleExpanded = (host: string, apiId: string, version: string) => {
    setExpandedDeployment(prev =>
      prev.includes(`${host}.${apiId}.${version}`)
        ? prev.filter(item => item !== `${host}.${apiId}.${version}`)
        : [...prev, `${host}.${apiId}.${version}`],
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
              {deployments.map(deployment => (
                <Card
                  key={deployment.site.host}
                  className="p-6 from-background to-muted bg-gradient-to-br border-border w-full cursor-pointer hover:shadow-lg"
                >
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
                            variant="ghost"
                            size="icon"
                            className="text-destructive hover:text-destructive"
                            onClick={e => {
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
                      {deployment.apiDefinitions.map(api => (
                        <div key={api.id} className="grid space-y-2">
                          <div className="flex justify-between">
                            <div className="flex items-center gap-4">
                              <span
                                className="relative rounded bg-muted p-1 font-mono text-sm cursor-pointer"
                                onClick={() =>
                                  navigate(
                                    `/apis/${api.id}/version/${api.version}`,
                                  )
                                }
                              >
                                {api.id} (v{api.version})
                              </span>

                              {(apiList.find(
                                a =>
                                  a.id === api.id && a.version === api.version,
                              )?.routes?.length || 0) > 0 && (
                                <button
                                  onClick={() =>
                                    toggleExpanded(
                                      deployment.site.host,
                                      api.id,
                                      api.version,
                                    )
                                  }
                                  className="p-1 hover:bg-accent rounded-md"
                                >
                                  <ChevronRight
                                    className={`w-4 h-4 text-muted-foreground transition-transform ${
                                      expandedDeployment.includes(
                                        `${deployment.site.host}.${api.id}.${api.version}`,
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
                            `${deployment.site.host}.${api.id}.${api.version}`,
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

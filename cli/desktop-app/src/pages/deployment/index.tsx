import { Card, CardContent } from "@/components/ui/card";
import { ChevronRight, Copy, Layers, Plus, Trash } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useEffect, useState } from "react";

import { API } from "@/service";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Deployment } from "@/types/deployments";
import ErrorBoundary from "@/components/errorBoundary";
import { HTTP_METHOD_COLOR } from "@/components/nav-route";
import { useNavigate, useParams } from "react-router-dom";
import { HttpApiDefinition } from "@/types/golemManifest";

const RoutesCard = ({
  apiId,
  version,
  apiList,
  host,
}: {
  apiId: string;
  version: string;
  host: string;
  apiList: HttpApiDefinition[];
}) => {
  const routes = apiList.find(
    api => api.id === apiId && api.version === version,
  )?.routes;
  const navigate = useNavigate();
  const { appId } = useParams();
  const [hoveredPath, setHoveredPath] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

  const copyToClipboard = (endpoint: { path: string; method: string }) => {
    const fullUrl = `${host}${endpoint.path}`;
    const method = endpoint.method.toUpperCase(); // Ensure proper capitalization (GET, POST, PUT, DELETE, etc.)
    const curlCommand = `curl --location --request ${method} http://${fullUrl} \
  --header "Content-Type: application/json" \
  --header "Accept: application/json" \
  --data '{}'`;

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
                  `/app/${appId}/apis/${apiId}/version/${version}/routes?path=${encodeURIComponent(endpoint.path)}&method=${endpoint.method}`,
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
                  {endpoint.path || "/"}
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
  const [apiList, setApiList] = useState<HttpApiDefinition[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [dialogOpenForHost, setDialogOpenForHost] = useState<string | null>(
    null,
  );
  const { appId } = useParams<{ appId: string }>();
  const [selectedDeploymentHost, setSelectedDeploymentHost] = useState<
    string | null
  >(null);

  useEffect(() => {
    const fetchDeployments = async () => {
      try {
        const uploadedDefinitions = await API.apiService.getUploadedDefinitions(
          appId!,
        );

        const nonDraftDefinitions = uploadedDefinitions
          .filter((def: HttpApiDefinition) => !def.draft)
          .map((def: HttpApiDefinition) => ({
            id: def.id,
            version: def.version,
            routes: def.routes || [],
          }));

        setApiList(nonDraftDefinitions);

        const deploymentResponse = await API.deploymentService.getDeploymentApi(
          appId!,
        );

        if (!deploymentResponse || deploymentResponse.length === 0) {
          setDeployments([]);
          return;
        }

        const uniqueDeployments = deploymentResponse.reduce(
          (acc: Deployment[], current) => {
            if (
              !acc.find((item: Deployment) => item.domain === current.domain)
            ) {
              acc.push(current);
            }
            return acc;
          },
          [],
        );

        setDeployments(uniqueDeployments);
      } catch (error) {
        console.error("Failed to fetch deployments:", error);
        setDeployments([]);
      }
    };

    fetchDeployments();
  }, [appId]);

  const handleDelete = async () => {
    if (!selectedDeploymentHost) return;

    try {
      await API.deploymentService.deleteDeployment(
        appId!,
        selectedDeploymentHost,
      );
      setDeployments(prev =>
        prev.filter(d => d.domain !== selectedDeploymentHost),
      );
    } catch (error) {
      console.error("Error deleting deployment:", error);
    } finally {
      setDialogOpenForHost(null);
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
        <div className="flex items-center justify-between mb-8">
          <div>
            <h1 className="text-2xl font-bold tracking-tight">
              API Deployments
            </h1>
            <p className="text-sm text-muted-foreground mt-1">
              Manage your API deployments across different environments
            </p>
          </div>
          <Button
            onClick={() => navigate(`/app/${appId}/deployments/create`)}
            type="button"
            className="gap-2"
          >
            <Plus className="w-4 h-4" />
            New Deployment
          </Button>
        </div>

        <div className="space-y-4">
          {deployments.length > 0 ? (
            <div className="grid gap-4 overflow-auto max-h-[calc(100vh-200px)]">
              {deployments.map(deployment => (
                <Card
                  key={deployment.domain}
                  className="border-2 hover:border-primary/50 transition-all duration-200"
                >
                  <div className="p-6 space-y-6">
                    {/* Header with host and delete button */}
                    <div className="flex items-start justify-between">
                      <div className="flex items-center gap-3">
                        <div className="p-2 rounded-lg bg-primary/10">
                          <Layers className="h-5 w-5 text-primary" />
                        </div>
                        <div>
                          <h2 className="text-lg font-semibold flex items-center gap-2">
                            {deployment.domain}
                          </h2>
                          <p className="text-sm text-muted-foreground">
                            {deployment.apiDefinitions.length} API
                            {deployment.apiDefinitions.length !== 1
                              ? "s"
                              : ""}{" "}
                            deployed
                          </p>
                        </div>
                      </div>

                      <Dialog
                        open={dialogOpenForHost === deployment.domain}
                        onOpenChange={isOpen => {
                          if (isOpen) {
                            setSelectedDeploymentHost(deployment.domain);
                            setDialogOpenForHost(deployment.domain);
                          } else {
                            setDialogOpenForHost(null);
                          }
                        }}
                      >
                        <DialogTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="text-destructive hover:text-destructive hover:bg-destructive/10"
                            onClick={e => {
                              e.stopPropagation();
                              setSelectedDeploymentHost(deployment.domain);
                              setDialogOpenForHost(deployment.domain);
                            }}
                          >
                            <Trash className="h-4 w-4" />
                          </Button>
                        </DialogTrigger>
                        <DialogContent>
                          <DialogHeader>
                            <DialogTitle>Delete Deployment</DialogTitle>
                            <DialogDescription>
                              Are you sure you want to delete the deployment at{" "}
                              <strong className="text-foreground">
                                {selectedDeploymentHost}
                              </strong>
                              ?
                              <br />
                              This action cannot be undone.
                            </DialogDescription>
                          </DialogHeader>
                          <DialogFooter>
                            <Button
                              variant="outline"
                              onClick={() => setDialogOpenForHost(null)}
                            >
                              Cancel
                            </Button>
                            <Button
                              variant="destructive"
                              onClick={handleDelete}
                            >
                              Delete Deployment
                            </Button>
                          </DialogFooter>
                        </DialogContent>
                      </Dialog>
                    </div>

                    {/* API Definitions List */}
                    <div className="space-y-3">
                      {deployment.apiDefinitions.map(apiDefString => {
                        // Parse "id@version" format
                        const [apiId = "", apiVersion = ""] =
                          apiDefString.split("@");
                        return (
                          <div key={apiDefString} className="space-y-2">
                            <div className="flex items-center justify-between p-3 rounded-lg border bg-card hover:bg-accent/50 transition-colors">
                              <div className="flex items-center gap-3 flex-1">
                                <Badge
                                  variant="secondary"
                                  className="font-mono"
                                >
                                  {apiId}
                                </Badge>
                                <Badge
                                  variant="outline"
                                  className="font-mono text-xs"
                                >
                                  v{apiVersion}
                                </Badge>
                                <span className="text-xs text-muted-foreground">
                                  {apiList.find(
                                    a =>
                                      a.id === apiId &&
                                      a.version === apiVersion,
                                  )?.routes?.length || 0}{" "}
                                  route
                                  {(apiList.find(
                                    a =>
                                      a.id === apiId &&
                                      a.version === apiVersion,
                                  )?.routes?.length || 0) !== 1
                                    ? "s"
                                    : ""}
                                </span>
                              </div>

                              {(apiList.find(
                                a => a.id === apiId && a.version === apiVersion,
                              )?.routes?.length || 0) > 0 && (
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  onClick={() =>
                                    toggleExpanded(
                                      deployment.domain,
                                      apiId,
                                      apiVersion,
                                    )
                                  }
                                  className="h-8 w-8 p-0"
                                >
                                  <ChevronRight
                                    className={cn(
                                      "h-4 w-4 transition-transform duration-200",
                                      expandedDeployment.includes(
                                        `${deployment.domain}.${apiId}.${apiVersion}`,
                                      ) && "rotate-90",
                                    )}
                                  />
                                </Button>
                              )}
                            </div>

                            {expandedDeployment.includes(
                              `${deployment.domain}.${apiId}.${apiVersion}`,
                            ) && (
                              <div className="pl-4">
                                <RoutesCard
                                  apiId={apiId}
                                  version={apiVersion}
                                  apiList={apiList}
                                  host={deployment.domain}
                                />
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          ) : (
            <Card className="border-2 border-dashed">
              <div className="flex flex-col items-center justify-center py-16">
                <div className="p-4 rounded-full bg-muted mb-4">
                  <Layers className="h-8 w-8 text-muted-foreground" />
                </div>
                <h3 className="text-lg font-semibold mb-2">
                  No Deployments Yet
                </h3>
                <p className="text-sm text-muted-foreground mb-6 text-center max-w-sm">
                  Create your first API deployment to start serving your APIs
                </p>
              </div>
            </Card>
          )}
        </div>
      </div>
    </ErrorBoundary>
  );
}

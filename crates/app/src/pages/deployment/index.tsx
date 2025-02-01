import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ChevronRight, Layers, Plus, Trash } from "lucide-react";
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
import { Api } from "@/types/api";
import { Deployment } from "@/types/deployments";
import ErrorBoundary from "@/components/errorBoundary";
import { useNavigate } from "react-router-dom";
import { removeDuplicateApis } from "@/lib/utils";

export default function Deployments() {
  const navigate = useNavigate();

  const [expandedDeployment, setExpandedDeployment] = useState([] as string[]);
  const [apiList, setApiList] = useState([] as Api[]);
  const [copied, setCopied] = useState(false);
  const [deployments, setDeployments] = useState([] as Deployment[]);
  const [isDialogOpen, setIsDialogOpen] = useState(false);
  const [isHovered, setIsHovered] = useState("");
  const [selectedDeploymentHost, setSelectedDeploymentHost] = useState<
    string | null
  >(null);

  useEffect(() => {
    const fetchDeployments = async () => {
      try {
        const response = await API.getApiList();
        setApiList(response);
        const newData = removeDuplicateApis(response);
        const deploymentPromises = newData.map((api) =>
          API.getDeploymentApi(api.id)
        );
        const allDeployments = await Promise.all(deploymentPromises);
        const combinedDeployments = allDeployments.flat().filter(Boolean);
        setDeployments(combinedDeployments);
      } catch (error) {
        console.error("Error fetching deployments:", error);
      }
    };

    fetchDeployments();
  }, []);

  const handleDelete = () => {
    if (!selectedDeploymentHost) return;
    API.deleteDeployment(selectedDeploymentHost)
      .then(() => {
        setDeployments((prev) =>
          prev.filter(
            (deployment) => deployment.site.host !== selectedDeploymentHost
          )
        );
        setIsDialogOpen(false);
        setSelectedDeploymentHost(null);
      })
      .catch((error) => {
        console.error("Error deleting deployment:", error);
        setIsDialogOpen(false);
      });
  };

  const copyToClipboard = () => {
    // TODO: Add support for Curl Message properly
    const curlCommand = `curl -X`;
    navigator.clipboard
      .writeText(curlCommand)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000); // Hide message after 2 seconds
      })
      .catch((err) => {
        console.error("Failed to copy:", err);
      });
  };

  return (
    <ErrorBoundary>
      <div className="p-6  mx-auto max-w-7xl">
        <div className="flex items-center justify-between mb-6">
          <h1 className="text-xl font-semibold">API Deployments</h1>
          <Button
            size="sm"
            onClick={() => {
              navigate("/deployments/create");
            }}
          >
            <Plus className="w-4 h-4 mr-2" />
            New
          </Button>
        </div>

        <div className="space-y-4">
          {deployments.length > 0 ? (
            <div className="grid gap-6 overflow-scroll max-h-[80vh]">
              {deployments.map((deployment, index) => (
                <Card
                  key={index}
                  className="p-6"
                  onMouseEnter={() => setIsHovered(deployment.site.host)}
                  onMouseLeave={() => {
                    setIsHovered("");
                    setCopied(false);
                  }}
                >
                  <div className="space-y-6">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center space-x-2">
                        <h2 className="text-base font-medium">
                          {deployment.site.host}
                        </h2>
                      </div>
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
                              Are you sure you want to delete the deployment for{" "}
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
                      {deployment.apiDefinitions.map((api, index) => (
                        <div
                          key={index}
                          className="grid items-center space-x-2 gap-2"
                        >
                          <div className="flex justify-between">
                            <div className="flex items-center gap-4 space-between">
                              <div
                                className="relative rounded bg-muted p-1 font-mono text-sm cursor-pointer"
                                onClick={() => {
                                  navigate(
                                    `/apis/${api.id}/version/${api.version}`
                                  );
                                }}
                              >
                                {api.id} (v{api.version})
                              </div>
                              {(
                                (
                                  apiList?.find(
                                    (data: Api) =>
                                      data.id === api.id &&
                                      data.version === api.version
                                  ) || {}
                                ).routes || []
                              )?.length > 0 && (
                                <button
                                  onClick={() => {
                                    if (
                                      expandedDeployment.includes(
                                        `${api.id}.${api.version}`
                                      )
                                    ) {
                                      setExpandedDeployment(
                                        expandedDeployment.filter(
                                          (item) =>
                                            item !== `${api.id}.${api.version}`
                                        )
                                      );
                                    } else {
                                      setExpandedDeployment([
                                        ...expandedDeployment,
                                        `${api.id}.${api.version}`,
                                      ]);
                                    }
                                  }}
                                  className="p-1 hover:bg-accent rounded-md"
                                >
                                  <ChevronRight
                                    className={`w-4 h-4 text-muted-foreground transition-transform ${
                                      expandedDeployment.includes(
                                        `${api.id}.${api.version}`
                                      )
                                        ? "rotate-90"
                                        : ""
                                    }`}
                                  />
                                </button>
                              )}
                            </div>
                            {isHovered === deployment.site.host && (
                              <div className="flex items-center space-x-3">
                                <div
                                  onClick={copyToClipboard}
                                  className="cursor-pointer hover:bg-accent hover:text-accent-foreground font-mono text-sm p-1"
                                >
                                  Copy Curl
                                </div>
                                {copied && (
                                  <span className="text-green-600 font-mono text-sm">
                                    ✅ Copied!
                                  </span>
                                )}
                              </div>
                            )}
                          </div>
                          {expandedDeployment.includes(
                            `${api.id}.${api.version}`
                          ) && (
                            <RoutesCard
                              apiId={api.id}
                              version={api.version}
                              apiList={apiList}
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
              <h3 className="text-lg font-medium mb-2">No Deployment</h3>
              <p className="text-muted-foreground mb-4">
                Create your first Deployment to get started
              </p>
            </div>
          )}
        </div>
      </div>
    </ErrorBoundary>
  );
}

const RoutesCard = ({
  apiId,
  version,
  apiList,
}: {
  apiId: string;
  version: string;
  apiList: Api[];
}) => {
  const routes = apiList.find(
    (api: Api) => api.id === apiId && api.version === version
  )?.routes;
  const navigate = useNavigate();

  return (
    routes && (
      <div className="space-y-2">
        {routes.map((endpoint, index) => (
          <div
            key={index}
            className="flex items-center space-x-2 cursor-pointer"
            onClick={() => {
              navigate(
                `/apis/${apiId}/version/${version}/routes?path=${endpoint.path}&method=${endpoint.method}`
              );
            }}
          >
            <span className="px-2 py-0.5 text-xs font-medium rounded bg-emerald-100 dark:bg-emerald-900 text-emerald-700 dark:text-emerald-200">
              {endpoint.method}
            </span>
            <code className="text-sm font-mono">{endpoint.path}</code>
          </div>
        ))}
      </div>
    )
  );
};

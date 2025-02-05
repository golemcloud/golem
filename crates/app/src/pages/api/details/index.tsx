import { useParams, useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
import { Trash2, Plus, Globe } from "lucide-react";
import { API } from "@/service";
import { Api, Route } from "@/types/api";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Deployment } from "@/types/deployments.ts";

const APIDetails = () => {
  const { apiName, version } = useParams();
  const navigate = useNavigate();
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  const [deployments, setDeployments] = useState([] as Deployment[]);

  useEffect(() => {
    if (apiName) {
      API.getApi(apiName).then((response) => {
        const selectedApi = response.find((api) => api.version === version);
        setActiveApiDetails(selectedApi!);
      });
      API.getDeploymentApi(apiName).then((response) => {
        const result = [] as Deployment[];
        response.forEach((deployment: Deployment) => {
          if (deployment.apiDefinitions.length > 0) {
            deployment.apiDefinitions.forEach((apiDefinition) => {
              if (apiDefinition.version === version) {
                result.push(deployment);
              }
            });
          }
        });
        setDeployments(result);
      });
    }
  }, [apiName, version]);

  const routeToQuery = (route: Route) => {
    navigate(
      `/apis/${apiName}/version/${version}/routes/?path=${route.path}&method=${route.method}`
    );
  };

  const handleDeleteRoute = (index: number) => {
    const newRoutes = [...activeApiDetails.routes];
    newRoutes.splice(index, 1);
    const newApiDetails = {
      ...activeApiDetails,
      routes: newRoutes,
    };
    API.putApi(
      activeApiDetails.id,
      activeApiDetails.version,
      newApiDetails
    ).then(() => {
      setActiveApiDetails(newApiDetails);
    });
  };

  return (
    <ErrorBoundary>
      <main className="flex-1 overflow-y-auto p-6 h-[80vh]">
        <section className="grid gap-16">
          <Card>
            <CardHeader>
              <div className="flex items-center justify-between">
                <CardTitle>Routes</CardTitle>
                <Button
                  variant="outline"
                  onClick={() => navigate(`/apis/${apiName}/routes/new`)}
                  className="flex items-center gap-2"
                >
                  <Plus className="h-5 w-5" />
                  <span>Add</span>
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              {activeApiDetails?.routes?.length === 0 ? (
                <div className="text-center">
                  No routes defined for this API version
                </div>
              ) : (
                <div className="space-y-4">
                  {activeApiDetails?.routes?.map((route, index) => (
                    <div
                      key={`${route.method}-${route.path}`}
                      className="flex items-center justify-between rounded-lg border p-4 hover:bg-muted/50 transition-colors cursor-pointer"
                      onClick={() => routeToQuery(route)}
                    >
                      <div className="space-y-2">
                        <div className="flex items-center gap-2">
                          <Badge variant="secondary">{route.method}</Badge>
                          <code className="text-sm font-semibold">
                            {route.path}
                          </code>
                        </div>
                      </div>
                      <Dialog>
                        <DialogTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="text-muted-foreground hover:text-destructive"
                          >
                            <Trash2 className="h-4 w-4" />
                            <span className="sr-only">Delete route</span>
                          </Button>
                        </DialogTrigger>
                        <DialogContent>
                          <DialogHeader>
                            <DialogTitle>Delete Route</DialogTitle>
                            <DialogDescription>
                              Are you sure you want to delete the route{" "}
                              {route.method} {route.path}? This action cannot be
                              undone.
                            </DialogDescription>
                          </DialogHeader>
                          <div className="flex justify-end gap-2">
                            <Button
                              variant="destructive"
                              onClick={() => handleDeleteRoute(index)}
                            >
                              Delete Route
                            </Button>
                          </div>
                        </DialogContent>
                      </Dialog>
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="flex flex-row items-center justify-between">
              <CardTitle>Active Deployments</CardTitle>
              {deployments.length > 0 && (
                <Button
                  variant="ghost"
                  className="text-primary"
                  onClick={() => navigate(`/deployments`)}
                >
                  View All
                </Button>
              )}
            </CardHeader>
            <CardContent>
              <div className="grid gap-4">
                {deployments.length > 0 ? (
                  deployments.map((deployment) => (
                    <div
                      key={deployment.createdAt}
                      className="flex items-center justify-between rounded-lg border p-4"
                    >
                      <div className="space-y-2">
                        <div className="flex items-center gap-2">
                          <Globe className="h-4 w-4" />
                          <span className="font-medium">
                            {deployment.site.host}
                          </span>
                        </div>
                      </div>
                    </div>
                  ))
                ) : (
                  <div className="text-center">
                    No routes defined for this API version
                  </div>
                )}
              </div>
            </CardContent>
          </Card>
        </section>
      </main>
    </ErrorBoundary>
  );
};

export default APIDetails;

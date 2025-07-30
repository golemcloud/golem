import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Globe, LayoutGrid, Plus, Route } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

import { API } from "@/service";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Deployment } from "@/types/deployments.ts";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { HTTP_METHOD_COLOR } from "@/components/nav-route.tsx";
import {
  HttpApiDefinition,
  HttpApiDefinitionRoute,
} from "@/types/golemManifest";

const APIDetails = () => {
  const { apiName, version, appId } = useParams();
  const [queryParams] = useSearchParams();
  const reload = queryParams.get("reload");
  const navigate = useNavigate();
  const [activeApiDetails, setActiveApiDetails] = useState(
    {} as HttpApiDefinition,
  );

  const [deployments, setDeployments] = useState([] as Deployment[]);

  useEffect(() => {
    if (apiName) {
      API.apiService.getApi(appId!, apiName).then(response => {
        const selectedApi = response.find(r => r.version == version);
        setActiveApiDetails(selectedApi!);
      });
      API.deploymentService.getDeploymentApi(appId!).then(response => {
        const result = [] as Deployment[];
        response.forEach((deployment: Deployment) => {
          if (deployment.apiDefinitions.length > 0) {
            deployment.apiDefinitions.forEach(apiDefinition => {
              if (apiDefinition.version === version) {
                result.push(deployment);
              }
            });
          }
        });
        setDeployments(result);
      });
    }
  }, [apiName, version, reload]);

  const routeToQuery = (route: HttpApiDefinitionRoute) => {
    navigate(
      `/app/${appId}/apis/${apiName}/version/${version}/routes/?path=${route.path}&method=${route.method}`,
    );
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
                  onClick={() =>
                    navigate(
                      `/app/${appId}/apis/${apiName}/version/${version}/routes/add?`,
                    )
                  }
                  className="flex items-center gap-2"
                >
                  <Plus className="h-5 w-5" />
                  <span>Add</span>
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              {activeApiDetails?.routes?.length === 0 ? (
                <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
                  <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                    <Route className="h-8 w-8 text-gray-400" />
                  </div>
                  <h2 className="text-xl font-semibold mb-2 text-center">
                    No routes defined for this API version
                  </h2>
                  <p className="text-gray-500 mb-6 text-center">
                    Create a new route, and it will be listed here.
                  </p>
                </div>
              ) : (
                <div className="space-y-4">
                  {activeApiDetails?.routes?.map(route => (
                    <div
                      key={`${route.method}-${route.path}`}
                      className="flex items-center justify-between rounded-lg border p-2 hover:bg-muted/50 transition-colors cursor-pointer"
                      onClick={() => routeToQuery(route)}
                    >
                      <div className="space-y-2">
                        <div className="flex items-center gap-2">
                          <Badge
                            variant="secondary"
                            className={
                              HTTP_METHOD_COLOR[
                                route.method as keyof typeof HTTP_METHOD_COLOR
                              ]
                            }
                          >
                            {route.method}
                          </Badge>
                          <code className="text-sm font-semibold">
                            {route.path || "/"}
                          </code>
                        </div>
                      </div>
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
                  onClick={() => navigate(`/app/${appId}/deployments`)}
                >
                  View All
                </Button>
              )}
            </CardHeader>
            <CardContent>
              <div className="grid gap-4">
                {deployments.length > 0 ? (
                  deployments.map(deployment => (
                    <div
                      key={deployment.createdAt + deployment.site.host}
                      className="flex items-center justify-between rounded-lg border p-4 cursor-pointer"
                      onClick={() => navigate(`/app/${appId}/deployments`)}
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
                  <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
                    <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                      <LayoutGrid className="h-8 w-8 text-gray-400" />
                    </div>
                    <h2 className="text-xl font-semibold mb-2 text-center">
                      No Active Deployments
                    </h2>
                    <p className="text-gray-500 mb-6 text-center">
                      Create a new Deployment, and it will be listed here.
                    </p>
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

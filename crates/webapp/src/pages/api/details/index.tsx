import { useParams, useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
// import {Globe, Link2 } from "lucide-react"
import { Trash2, Plus } from "lucide-react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import ApiLeftNav from "./apiLeftNav.tsx";
import { API } from "@/service";
import { Api } from "@/types/api";
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

const APIDetails = () => {
  const { apiName } = useParams();
  const navigate = useNavigate();
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);
  // const [deployments] = useState([
  //   {
  //     domain: "api.golem.cloud",
  //     id: "abcd",
  //     status: "Active",
  //   },
  // ]);

  useEffect(() => {
    if (apiName) {
      API.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

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
      <div className="flex">
        <ApiLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {apiName}
                </h1>
                <div className="flex items-center gap-2">
                  {activeApiDetails.version && (
                    <Select
                      defaultValue={activeApiDetails.version}
                      onValueChange={(version) => {
                        const selectedApi = apiDetails.find(
                          (api) => api.version === version
                        );
                        if (selectedApi) {
                          setActiveApiDetails(selectedApi);
                        }
                      }}
                    >
                      <SelectTrigger className="w-28">
                        <SelectValue>{activeApiDetails.version}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {apiDetails.map((api) => (
                          <SelectItem value={api.version} key={api.version}>
                            {api.version}{" "}
                            {api.draft ? "(Draft)" : "(Published)"}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                </div>
              </div>
            </div>
          </header>

          <main className="flex-1 overflow-y-auto p-6 h-[80vh]">
            <section className="grid gap-16">
              <Card>
                <CardHeader>
                  <div className="flex items-center justify-between mb-4">
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
                          className="flex items-center justify-between rounded-lg border p-4 hover:bg-muted/50 transition-colors"
                        >
                          <div className="space-y-2">
                            <div className="flex items-center gap-2">
                              <Badge variant="secondary">{route.method}</Badge>
                              <code className="text-sm font-semibold">
                                {route.path}
                              </code>
                            </div>
                            <div className="space-y-1 text-sm text-muted-foreground">
                              <p className="text-xs">
                                Component ID:{" "}
                                {route.binding.componentId.componentId}
                              </p>
                              <p className="text-xs">
                                Version ID: {route.binding.componentId.version}
                              </p>
                              <p className="text-xs">
                                Worker: {route.binding.workerName}
                              </p>
                              <p className="text-xs">
                                Response: {route.binding.response || "N/A"}
                              </p>
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
                                  {route.method} {route.path}? This action
                                  cannot be undone.
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
              {/* <Card>
                <CardHeader className="flex flex-row items-center justify-between">
                  <CardTitle>Active Deployments</CardTitle>
                  <Button variant="ghost" className="text-primary">
                    View All
                  </Button>
                </CardHeader>
                <CardContent>
                  {deployments.map((deployment) => (
                    <div
                      key={deployment.id}
                      className="flex items-center justify-between rounded-lg border p-4"
                    >
                      <div className="space-y-2">
                        <div className="flex items-center gap-2">
                          <Globe className="h-4 w-4" />
                          <span className="font-medium">
                            {deployment.domain}
                          </span>
                        </div>
                        <div className="flex items-center gap-2 text-sm text-muted-foreground">
                          <Link2 className="h-4 w-4" />
                          <span>{deployment.id}</span>
                        </div>
                      </div>
                      <Badge variant="outline">{deployment.status}</Badge>
                    </div>
                  ))}
                </CardContent>
              </Card> */}
            </section>
          </main>
        </div>
      </div>
    </ErrorBoundary>
  );
};

export default APIDetails;

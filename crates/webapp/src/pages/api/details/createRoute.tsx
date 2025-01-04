import { useState, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { API } from "@/service";
import { Api } from "@/types/api";
import { Component } from "@/types/component";
import ErrorBoundary from "@/components/errorBoundary";
import ApiLeftNav from "./apiLeftNav";

const HTTP_METHODS = [
  "Get",
  "Post",
  "Put",
  "Patch",
  "Delete",
  "Head",
  "Options",
  "Trace",
  "Connect",
];

const CreateRoute = () => {
  const navigate = useNavigate();
  const { apiName } = useParams();
  const [method, setMethod] = useState("Get");
  const [path, setPath] = useState("");
  const [componentId, setComponentId] = useState("");
  const [version, setVersion] = useState("");
  const [workerName, setWorkerName] = useState("");
  const [response, setResponse] = useState("");
  const [componentList, setComponentList] = useState<{
    [key: string]: Component;
  }>({});
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  const [errors, setErrors] = useState({
    path: "",
    componentId: "",
    version: "",
    workerName: "",
  });

  useEffect(() => {
    if (apiName) {
      API.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

  useEffect(() => {
    API.getComponentByIdAsKey().then((response) => {
      setComponentList(response);
    });
  }, []);

  const validateForm = () => {
    const newErrors = {
      path: path ? "" : "Path is required.",
      componentId: componentId ? "" : "Component is required.",
      version: version ? "" : "Version is required.",
      workerName: workerName ? "" : "Worker Name is required.",
    };
    setErrors(newErrors);
    return Object.values(newErrors).every((error) => error === "");
  };

  const onCreateRoute = (event: React.FormEvent) => {
    event.preventDefault();

    if (!validateForm()) {
      return;
    }

    const payload = {
      id: activeApiDetails.id,
      version: activeApiDetails.version,
      draft: activeApiDetails.draft,
      routes: [
        ...activeApiDetails.routes,
        {
          method,
          path,
          binding: {
            componentId: { componentId, version },
            workerName,
            response,
          },
        },
      ],
    };

    API.putApi(activeApiDetails.id, activeApiDetails.version, payload).then(
      () => {
        navigate(`/apis/${apiName}`);
      }
    );
  };

  return (
    <ErrorBoundary>
      <div className="flex bg-background text-foreground">
        <ApiLeftNav />
        <div className="flex-1">
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
          <div className="overflow-y-auto h-[80vh] ">
            <div className="max-w-4xl mx-auto p-8">
              <div className="flex items-center gap-2 mb-8">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => navigate(`/apis/${apiName}`)}
                >
                  <ArrowLeft className="mr-2" />
                  Back
                </Button>
                Create New Route
              </div>
              <form className="space-y-8" onSubmit={onCreateRoute}>
                <div>
                  <h3 className="text-lg font-medium">HTTP Endpoint</h3>
                  <p className="text-sm text-gray-500 dark:text-gray-400">
                    Each API Route must have a unique Method + Path combination.
                  </p>
                  <div className="space-y-4 mt-4">
                    <div>
                      <Label>Method</Label>
                      <div className="flex flex-wrap gap-2 mt-2">
                        {HTTP_METHODS.map((m) => (
                          <Button
                            type="button"
                            key={m}
                            variant={method === m ? "default" : "outline"}
                            size="sm"
                            onClick={() => setMethod(m)}
                          >
                            {m}
                          </Button>
                        ))}
                      </div>
                    </div>
                    <div>
                      <Label>Path</Label>
                      <Input
                        type="text"
                        value={path}
                        onChange={(e) => setPath(e.target.value)}
                        placeholder="Define path variables with curly brackets (<VARIABLE_NAME>)"
                        className={` mt-2 ${
                          errors.path ? "border-destructive" : ""
                        }`}
                      />
                      {errors.path && (
                        <p className="text-sm text-red-500 mt-2">
                          {errors.path}
                        </p>
                      )}
                    </div>
                  </div>
                </div>

                <div>
                  <h3 className="text-lg font-medium">Worker Binding</h3>
                  <p className="text-sm text-gray-500 dark:text-gray-400">
                    Bind this endpoint to a specific worker function.
                  </p>
                  <div className="grid grid-cols-2 gap-4 mt-4">
                    <div>
                      <Label>Component</Label>
                      <Select
                        onValueChange={(componentId) =>
                          setComponentId(componentId)
                        }
                      >
                        <SelectTrigger
                          className={` ${
                            errors.componentId ? "border-destructive" : ""
                          }`}
                        >
                          <SelectValue placeholder="Select a component" />
                        </SelectTrigger>
                        <SelectContent>
                          {Object.values(componentList).map(
                            (data: Component) => (
                              <SelectItem
                                value={data.componentId || ""}
                                key={data.componentName}
                              >
                                {data.componentName}
                              </SelectItem>
                            )
                          )}
                        </SelectContent>
                      </Select>
                      {errors.componentId && (
                        <p className="text-sm text-red-500 mt-2">
                          {errors.componentId}
                        </p>
                      )}
                    </div>
                    <div>
                      <Label>Version</Label>
                      <Select onValueChange={(version) => setVersion(version)}>
                        <SelectTrigger
                          className={` ${
                            errors.version ? "border-destructive" : ""
                          }`}
                        >
                          <SelectValue placeholder="Select a version">
                            V{version}
                          </SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {componentId &&
                            componentList[componentId]?.versionId?.map(
                              (v: string) => (
                                <SelectItem value={v} key={v}>
                                  V{v}
                                </SelectItem>
                              )
                            )}
                        </SelectContent>
                      </Select>
                      {errors.version && (
                        <p className="text-sm text-red-500 mt-2">
                          {errors.version}
                        </p>
                      )}
                    </div>
                  </div>
                  <div className="mt-4">
                    <Label>Worker Name</Label>
                    <Textarea
                      value={workerName}
                      onChange={(e) => setWorkerName(e.target.value)}
                      placeholder="Interpolate variables into your Worker ID"
                      className={`mt-2 ${
                        errors.workerName ? "border-destructive" : ""
                      }`}
                    />
                    {errors.workerName && (
                      <p className="text-sm text-red-500 mt-2">
                        {errors.workerName}
                      </p>
                    )}
                  </div>
                </div>

                <div>
                  <h3 className="text-lg font-medium">Response</h3>
                  <p className="text-sm text-gray-500 dark:text-gray-400">
                    Define the HTTP response for this API Route.
                  </p>
                  <Textarea
                    value={response}
                    onChange={(e) => setResponse(e.target.value)}
                    className="mt-4"
                  />
                </div>

                <div className="flex justify-end space-x-3">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => {
                      setMethod("Get");
                      setPath("");
                      setVersion("");
                      setWorkerName("");
                      setResponse("");
                      setErrors({
                        path: "",
                        componentId: "",
                        version: "",
                        workerName: "",
                      });
                    }}
                  >
                    Clear
                  </Button>
                  <Button type="submit" variant="default">
                    Create Route
                  </Button>
                </div>
              </form>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
};

export default CreateRoute;

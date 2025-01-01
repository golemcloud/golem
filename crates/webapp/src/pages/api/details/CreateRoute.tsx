/* eslint-disable @typescript-eslint/no-explicit-any */
import { useState, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Slash } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { invoke } from "@tauri-apps/api/core";
import APILeftNav from "./apiLeftNav";
import { Button } from "@/components/ui/button";
import { toast } from "@/hooks/use-toast";

const ApiMockData = [
  {
    createdAt: "2024-12-31T05:34:20.197542+00:00",
    draft: false,
    id: "vvvvv",
    routes: [],
    version: "0.1.0",
  },
  {
    createdAt: "2025-01-01T08:50:03.144928+00:00",
    draft: true,
    id: "vvvvv",
    routes: [],
    version: "0.2.0",
  },
];

const ComponentMockData = {
  ["17c50abc-d410-4603-a0d7-97d1a05cbad2"]: {
    componentName: "Component",
    componentId: "17c50abc-d410-4603-a0d7-97d1a05cbad2",
    versionId: [0, 1, 2],
  },
};

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
  const [componentList, setComponentList] = useState(ComponentMockData as any);

  const [apiDetails, setApiDetails] = useState(ApiMockData);
  const [activeApiDetails, setActiveApiDetails] = useState(
    apiDetails[apiDetails.length - 1]
  );

  useEffect(() => {
    const fetchData = async () => {
      //check the api https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa?api-definition-id=${appId}
      // eslint-disable-next-line @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars
      const response: any = await invoke("get_api");
      setApiDetails(response);
      setActiveApiDetails(response[response.length - 1]);
    };
    fetchData().then((r) => r);

    const fetchData2 = async () => {
      // https://release.api.golem.cloud/v1/components?project-id=305e832c-f7c1-4da6-babc-cb2422e0f5aa
      //Get method
      // eslint-disable-next-line @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars
      const response: any = await invoke("get_api");
      const componentData = {} as any;
      response.forEach((data: any) => {
        componentData[data.versionedComponentId.componentId] = {
          componentName: data.componentName,
          componentId: data.versionedComponentId.componentId,
          versionId: [
            ...(componentData[data.versionedComponentId.componentId]
              .versionId || []),
            data.versionedComponentId.version,
          ],
        };
      });
      setComponentList(componentData);
    };
    fetchData2().then((r) => r);
  }, []);

  const onCreateRoute = () => {
    try {
      // Simulate API call
      //https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa/vvvvv/{activeApiDetails.version}
      //put method
      const payload = {
        id: activeApiDetails.id,
        version: activeApiDetails.version,
        draft: activeApiDetails.draft,
        routes: [
          ...activeApiDetails.routes,
          {
            method: method,
            path: path,
            binding: {
              componentId: {
                componentId: componentId,
                version: version,
              },
              workerName: workerName,
              response: response,
            },
          },
        ],
      };
      console.log(payload);
      navigate(`/apis/${apiName}`);
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
    } catch (error) {
      toast({
        variant: "destructive",
        title: "Error",
        description: "API failed .",
      });
    }
  };

  return (
    <div className="flex">
      <APILeftNav />
      <div className="flex-1">
        <div className="flex items-center justify-between">
          <header className="w-full border-b bg-background py-2">
            <div className="max-w-7xl px-6 lg:px-8">
              <div className="mx-auto max-w-2xl lg:max-w-none">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <h1 className="line-clamp-1 font-medium leading-tight sm:leading-normal">
                      {apiName}
                    </h1>
                    <div className="flex items-center gap-1">
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
                        <SelectTrigger className="w-20 h-6">
                          <SelectValue placeholder="Version">
                            {activeApiDetails.version}
                          </SelectValue>
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
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </header>
        </div>
        <div className="overflow-scroll h-[95vh] p-8 max-w-4xl mx-auto">
          <div className="flex items-center mb-6 border-b border-gray-300 pb-4">
            <button
              onClick={() => navigate(`/apis/${apiName}`)}
              className="text-xl  flex items-center text-gray-800 hover:text-gray-900"
            >
              <ArrowLeft className="h-4 w-4 mr-2" />
            </button>
            <span>New Route</span>
          </div>

          <form className="space-y-8  p-6">
            <section>
              <h3 className="text-lg font-medium mb-4">HTTP Endpoint</h3>
              <p className="text-sm text-gray-600 mb-4">
                Each API Route must have a unique Method + Path combination
              </p>

              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">
                    Method
                  </label>
                  <div className="flex flex-wrap gap-2">
                    {HTTP_METHODS.map((m) => (
                      <button
                        key={m}
                        type="button"
                        onClick={() => setMethod(m)}
                        className={`px-3 py-1 rounded border hover:border-gray-400 ${
                          method === m
                            ? "bg-gray-200 text-gray-900 border-gray-400"
                            : "text-gray-600 hover:bg-gray-50 border-gray-200"
                        }`}
                      >
                        {m}
                      </button>
                    ))}
                  </div>
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">
                    Path
                  </label>
                  <input
                    type="text"
                    value={path}
                    onChange={(e) => setPath(e.target.value)}
                    placeholder="Define path variables with curly brackets (<VARIABLE_NAME>)"
                    className="w-full px-3 py-2 border border-gray-200 rounded-md"
                  />
                </div>
              </div>
            </section>

            <section>
              <h3 className="text-lg font-medium mb-4">Worker Binding</h3>
              <p className="text-sm text-gray-600 mb-4">
                Bind this endpoint to a specific worker function
              </p>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">
                    Component
                  </label>
                  <Select
                    onValueChange={(componentId) => {
                      setComponentId(componentId);
                    }}
                  >
                    <SelectTrigger className="w-full h-10">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {Object.values(componentList).map((data: any) => (
                        <SelectItem
                          value={data.componentId}
                          key={data.componentName}
                        >
                          {data.componentName}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div>
                  <label className="block text-sm ml-14 font-medium text-gray-700 mb-2">
                    Version
                  </label>
                  <div className="flex items-center gap-8">
                    <Slash className="h-10 w-7" />
                    <Select
                      onValueChange={(version) => {
                        setVersion(version);
                      }}
                    >
                      <SelectTrigger className="w-full h-10">
                        <SelectValue>V{version} </SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {componentList[componentId]?.versionId.map(
                          (data: any) => (
                            <SelectItem value={data} key={data}>
                              V{data}
                            </SelectItem>
                          )
                        )}
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </div>

              <div className="mt-4">
                <label className="block text-sm font-medium text-gray-700 mb-2">
                  Worker Name
                </label>
                <textarea
                  value={workerName}
                  onChange={(e) => setWorkerName(e.target.value)}
                  placeholder="Interpolate variables into your Worker ID"
                  className="w-full px-3 py-2 border border-gray-200 rounded-md h-24"
                />
              </div>
            </section>

            <section>
              <h3 className="text-lg font-medium mb-4">Response</h3>
              <p className="text-sm text-gray-600 mb-4">
                Define the HTTP response for this API Route
              </p>

              <textarea
                value={response}
                onChange={(e) => setResponse(e.target.value)}
                className="w-full px-3 py-2 border border-gray-200 rounded-md h-32"
              />
            </section>

            <div className="flex justify-end space-x-3">
              <Button
                type="button"
                onClick={() => {
                  setMethod("Get");
                  setPath("");
                  setComponentId("");
                  setVersion("");
                  setWorkerName("");
                  setResponse("");
                }}
                className="px-4 py-2"
                variant={"secondary"}
              >
                Clear
              </Button>
              <Button
                type="submit"
                onClick={onCreateRoute}
                disabled={
                  !path || !method || !componentId || !version || !workerName
                }
                className="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700"
              >
                Create Route
              </Button>
            </div>
          </form>
        </div>
      </div>
    </div>
  );
};

export default CreateRoute;

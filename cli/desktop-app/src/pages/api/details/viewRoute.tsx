// import { Api, RouteRequestData } from "@/types/api";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

import { API } from "@/service";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
// import { CorsDisplay } from "@/components/cors-display";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { HTTP_METHOD_COLOR } from "@/components/nav-route";
import { RibEditor } from "@/components/rib-editor";
import { useToast } from "@/hooks/use-toast";
import {
  HttpApiDefinition,
  HttpApiDefinitionRoute,
} from "@/types/golemManifest";

interface CodeBlockProps {
  code: string | string[];
  label?: string;
  allowCopy?: boolean;
}

function CodeBlock({ code, label, allowCopy = false }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);
  const { toast } = useToast();

  const copyToClipboard = async () => {
    const textToCopy = Array.isArray(code) ? code.join("\n") : code;
    try {
      await navigator.clipboard.writeText(textToCopy);
      setCopied(true);
      toast({
        description: "Code copied to clipboard",
        duration: 2000,
      });
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast({
        variant: "destructive",
        description: "Failed to copy code",
        duration: 2000,
      });
    }
  };

  return (
    <div className="relative group">
      {allowCopy && (
        <div className="absolute right-2 top-2 opacity-0 group-hover:opacity-100 transition-opacity">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 bg-gray-200 dark:bg-gray-800/50 hover:bg-gray-300 dark:hover:bg-gray-800"
            onClick={copyToClipboard}
            aria-label={`Copy ${label || "code"}`}
          >
            {copied ? (
              <CheckIcon className="h-4 w-4 text-green-600 dark:text-green-400" />
            ) : (
              <CopyIcon className="h-4 w-4 text-gray-600 dark:text-gray-400" />
            )}
          </Button>
        </div>
      )}
      <pre className="bg-gray-100 dark:bg-[#161B22] rounded p-3 font-mono text-sm text-gray-800 dark:text-gray-300 overflow-x-auto">
        <code>
          {Array.isArray(code)
            ? code.map((line, index) => (
                <div key={index} className="py-1">
                  {line}
                </div>
              ))
            : code}
        </code>
      </pre>
    </div>
  );
}

function PathParameters({ url }: { url: string }) {
  const [parameters, setParameters] = useState<
    { name: string; type: string }[]
  >([]);
  const extractDynamicParams = (path: string) => {
    const pathParamRegex = /{([^}]+)}/g; // Matches {param} in path
    const queryParamRegex = /[?&]([^=]+)={([^}]+)}/g; // Matches ?key={param} or &key={param}

    const params: { name: string; type: string }[] = [];
    let match;

    // Extract path parameters
    while ((match = pathParamRegex.exec(path)) !== null) {
      params.push({ name: match[1]!, type: "path" });
    }

    // Extract query parameters (key-value pair)
    while ((match = queryParamRegex.exec(path)) !== null) {
      params.push({ name: match[1]!, type: "query" });
    }
    setParameters(params);
  };
  useEffect(() => {
    extractDynamicParams(url);
  }, [url]);

  return (
    <div className="bg-gray-100 dark:bg-[#161B22] rounded p-3 overflow-x-auto">
      <div className="flex flex-wrap gap-2">
        {parameters.map(param => (
          <Badge
            key={param.name}
            variant="outline"
            className={`font-mono text-sm ${
              param.type === "path"
                ? "border-blue-500 dark:border-blue-500"
                : "border-gray-500 dark:border-gray-500"
            }`}
          >
            <span className="text-purple-600 dark:text-purple-400">
              {param.name}
            </span>
            <span className="text-gray-500 dark:text-gray-400">: </span>
            <span className="text-yellow-600 dark:text-yellow-300">
              {param.type}
            </span>
          </Badge>
        ))}
      </div>
    </div>
  );
}

export const ApiRoute = () => {
  const navigate = useNavigate();
  const { apiName, version, appId } = useParams();
  const [currentRoute, setCurrentRoute] = useState(
    {} as HttpApiDefinitionRoute,
  );
  const [_apiResponse, setApiResponse] = useState({} as HttpApiDefinition);
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const reload = queryParams.get("reload");

  useEffect(() => {
    const fetchData = async () => {
      if (apiName && version && method && path !== null) {
        const apiResponse = await API.apiService.getApi(appId!, apiName);
        const selectedApi = apiResponse.find(api => api.version === version);
        if (selectedApi) {
          setApiResponse(selectedApi);
          const route = selectedApi.routes?.find(
            route => route.path === path && route.method === method,
          );
          setCurrentRoute(route || ({} as HttpApiDefinitionRoute));
        } else {
          navigate(`/app/${appId}/apis/${apiName}/version/${version}`);
        }
      } else {
        navigate(`/app/${appId}/apis/${apiName}/version/${version}`);
      }
    };
    fetchData();
  }, [apiName, version, path, method, reload]);

  // const routeToQuery = () => {
  //   navigate(
  //     `/app/${appId}/apis/${apiName}/version/${version}/routes/edit?path=${path}&method=${method}`,
  //   );
  // };

  // const handleDelete = () => {
  //   if (apiName) {
  //     API.getApi(appId!, apiName).then(async response => {
  //       const currentApi = response.find(api => api.version === version);
  //       if (currentApi) {
  //         currentApi.routes = currentApi.routes?.filter(
  //           route => !(route.path === path && route.method === method),
  //         );
  //         API.putApi(apiName, version!, currentApi).then(() => {
  //           navigate(`/app/${appId}/apis/${apiName}/version/${version}`);
  //         });
  //       }
  //     });
  //   }
  // };

  return (
    <ErrorBoundary>
      <main className=" mx-auto p-6 w-full max-w-7xl">
        <Card className="w-full text-gray-800 dark:text-gray-200 p-6 border-gray-200 dark:border-gray-800">
          <CardHeader>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Badge
                  variant="secondary"
                  className={
                    HTTP_METHOD_COLOR[
                      currentRoute.method as keyof typeof HTTP_METHOD_COLOR
                    ]
                  }
                >
                  {currentRoute.method}
                </Badge>
                <span className="font-mono text-gray-600 dark:text-gray-300">
                  {currentRoute.path || "/"}
                </span>
              </div>
              {/* {apiResponse?.draft && (
                <div className="flex gap-2 items-center">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-gray-600 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200"
                    onClick={() => routeToQuery()}
                  >
                    <Edit2Icon className="h-4 w-4 mr-1" />
                    Edit
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-red-600 dark:text-red-400 hover:text-red-800 dark:hover:text-red-200"
                    onClick={handleDelete}
                  >
                    <Trash2Icon className="h-4 w-4 mr-1" />
                    Delete
                  </Button>
                </div>
              )} */}
            </div>
          </CardHeader>
          <CardContent className="space-y-6 pt-6">
            {currentRoute?.binding?.componentName && (
              <div className="mb-6">
                <h2 className="text-gray-800 dark:text-gray-200 mb-2">
                  Component
                </h2>
                <CodeBlock
                  code={`${
                    currentRoute?.binding?.componentName
                  } / v${currentRoute?.binding?.componentVersion}`}
                  label="component name"
                  allowCopy={false}
                />
              </div>
            )}

            {/* Path Section */}
            {currentRoute?.path && (
              <div className="mb-6">
                <div className="flex items-center gap-2 mb-2">
                  <h2 className="text-gray-800 dark:text-gray-200">
                    Parameters
                  </h2>
                </div>
                <PathParameters url={currentRoute?.path} />
              </div>
            )}

            {/* Worker Name Section */}
            {/* {currentRoute?.binding?.workerName && (
              <div>
                <div className="flex items-center gap-2 mb-2">
                  <h2 className="text-gray-800 dark:text-gray-200">
                    Worker Name
                  </h2>
                  <span className="text-blue-600 dark:text-blue-400 text-sm border border-blue-300 dark:border-blue-500/30 rounded px-2 py-0.5">
                    Rib
                  </span>
                </div>
                <RibEditor
                  value={currentRoute?.binding?.workerName}
                  disabled={true}
                />
                {/* <CodeBlock
                  code={currentRoute?.binding?.workerName || "No worker name"}
                  label="worker name RIB script"
                  allowCopy={true}
                /> 
                </div> */}

            {/* Response Section */}
            {currentRoute?.binding?.response && (
              <div className="mb-6">
                <div className="flex items-center gap-2 mb-2">
                  <h2 className="text-gray-800 dark:text-gray-200">Response</h2>
                  <span className="text-blue-600 dark:text-blue-400 text-sm border border-blue-300 dark:border-blue-500/30 rounded px-2 py-0.5">
                    Rib
                  </span>
                </div>
                <RibEditor
                  value={currentRoute?.binding?.response}
                  disabled={true}
                />
              </div>
            )}

            {/* Cors Section */}
            {currentRoute?.binding?.type == "cors-preflight" &&
              // <CorsDisplay cors={currentRoute?.binding?.corsPreflight} />
              // <div className="mb-6">
              //   <div className="flex items-center gap-2 mb-2">
              //     <h2 className="text-gray-800 dark:text-gray-200">Response</h2>
              //     <span className="text-blue-600 dark:text-blue-400 text-sm border border-blue-300 dark:border-blue-500/30 rounded px-2 py-0.5">
              //       Rib
              //     </span>
              //   </div>
              //   <RibEditor
              //     value={currentRoute?.binding?.response}
              //     disabled={true}
              //   />
              // </div>
              ""}
          </CardContent>
        </Card>
      </main>
    </ErrorBoundary>
  );
};

import {
  useLocation,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router-dom";
import {
  ArrowLeft,
  CircleFadingPlusIcon,
  Home,
  Plus,
  Settings,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge.tsx";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { Api, Route } from "@/types/api.ts";

export const HTTP_METHOD_COLOR = {
  Get: "bg-emerald-900 text-emerald-200 hover:bg-emerald-900",
  Post: "bg-gray-900 text-gray-200 hover:bg-gray-900",
  Put: "bg-yellow-900 text-yellow-200 hover:bg-yellow-900",
  Patch: "bg-blue-900 text-blue-200 hover:bg-blue-900",
  Delete: "bg-red-900 text-red-200 hover:bg-red-900",
  Head: "bg-purple-900 text-purple-200 hover:bg-purple-900",
  Options: "bg-indigo-900 text-indigo-200 hover:bg-indigo-900",
  Trace: "bg-pink-900 text-pink-200 hover:bg-pink-900",
  Connect: "bg-sky-900 text-sky-200 hover:bg-sky-900",
};

const ApiLeftNav = () => {
  const navigate = useNavigate();
  const { apiName, version } = useParams();
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const location = useLocation();
  const [apiDetails, setApiDetails] = useState({} as Api);

  const isActive = (path: string) => location.pathname.endsWith(path);

  useEffect(() => {
    API.getApi(apiName!).then(async (response) => {
      const selectedApi = response.find((api) => api.version === version);
      if (selectedApi) {
        setApiDetails(selectedApi);
      }
    });
  }, [apiName, version, path, method]);

  const routeToQuery = (route: Route) => {
    navigate(
      `/apis/${apiName}/version/${version}/routes/?path=${route.path}&method=${route.method}`
    );
  };

  return (
    <ErrorBoundary>
      <nav className="w-64 border-r p-4 border-gray-200 dark:border-gray-700 min-h-[94vh]">
        <div className="mb-6">
          <div className="flex items-center mb-6">
            <div onClick={() => navigate(-1)}>
              <ArrowLeft className="h-5 w-5 mr-2 text-gray-800 dark:text-gray-200 hover:text-gray-600 dark:hover:text-gray-400 cursor-pointer" />
            </div>
            <h1 className="text-lg font-semibold text-gray-800 dark:text-gray-200">
              API
            </h1>
          </div>

          <ul className="space-y-1">
            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/apis/${apiName}/version/${version}`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive(apiName ?? "")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Home className="h-5 w-5 mr-3" />
                <span>Overview</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(`/apis/${apiName}/version/${version}/settings`)
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("settings")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Settings className="h-5 w-5 mr-3" />
                <span>Settings</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(`/apis/${apiName}/version/${version}/newversion`)
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("newversion")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <CircleFadingPlusIcon className="h-5 w-5 mr-3" />
                <span>New version</span>
              </Button>
            </li>
          </ul>
        </div>

        <div>
          <h2 className="text-sm font-medium text-gray-500 dark:text-gray-400 mb-3">
            Routes
          </h2>
          <div className="grid text-sm font-medium text-gray-500 dark:text-gray-400 mb-3 gap-2">
            {apiDetails?.routes?.map((route) => (
              <div
                key={`${route.method}-${route.path}`}
                className={`flex items-center gap-2 cursor-pointer ${
                  path === route.path && method === route.method
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                }`}
                onClick={() => {
                  routeToQuery(route);
                }}
              >
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
                <span className="text-sm font-mono">{route.path}</span>
              </div>
            ))}
          </div>
          <Button
            variant="outline"
            onClick={() =>
              navigate(`/apis/${apiName}/version/${version}/routes/add?`)
            }
            className="flex items-center justify-center text-sm px-3 py-2 w-full rounded-lg border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-100 hover:border-gray-400 dark:hover:border-gray-500"
          >
            <Plus className="h-5 w-5" />
            <span>Add</span>
          </Button>
        </div>
      </nav>
    </ErrorBoundary>
  );
};

export default ApiLeftNav;

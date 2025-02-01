import { useNavigate, useParams, useLocation } from "react-router-dom";
import {
  ArrowRightFromLine,
  Home,
  Pencil,
  Settings,
  ArrowLeft,
  Pickaxe,
  Info,
  Workflow,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import ErrorBoundary from "@/components/errorBoundary";
import { Component } from "@/types/component";

interface ComponentLeftNavProps {
  componentDetails: Component;
}

const ComponentLeftNav: React.FC<ComponentLeftNavProps> = ({
  componentDetails,
}) => {
  const navigate = useNavigate();
  const { componentId } = useParams();
  const location = useLocation();

  const isActive = (path: string) => location.pathname.endsWith(path);

  return (
    <ErrorBoundary>
      <nav className="w-64 border-r p-4 border-gray-200 dark:border-gray-700 min-h-[88vh]">
        <div className="mb-6">
          <div className="flex items-center mb-6">
            <div onClick={() => navigate(-1)}>
              <ArrowLeft className="h-5 w-5 mr-2 text-gray-800 dark:text-gray-200 hover:text-gray-600 dark:hover:text-gray-400 cursor-pointer" />
            </div>
            <h1 className="text-lg font-semibold text-gray-800 dark:text-gray-200">
              Component
            </h1>
          </div>

          <ul className="space-y-1">
            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/components/${componentId}`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive(componentId ?? "")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Home className="h-5 w-5 mr-2" />
                <span>Overview</span>
              </Button>
            </li>
            {componentDetails?.componentType === "Durable" ||
            !componentDetails?.componentType ? (
              <li>
                <Button
                  variant="ghost"
                  onClick={() => navigate(`/components/${componentId}/workers`)}
                  className={cn(
                    "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                    isActive("workers") || isActive("create")
                      ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                      : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                  )}
                >
                  <Pickaxe className="h-5 w-5 mr-2" />
                  <span>Workers</span>
                </Button>
              </li>
            ) : (
              <li>
                <Button
                  variant="ghost"
                  onClick={() => navigate(`/components/${componentId}/invoke`)}
                  className={cn(
                    "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                    isActive("invoke")
                      ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                      : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                  )}
                >
                  <Workflow className="h-5 w-5 mr-2" />
                  <span>Invoke</span>
                </Button>
              </li>
            )}

            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/components/${componentId}/exports`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("exports")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <ArrowRightFromLine className="h-4 w-4 mr-2" />
                <span>Exports</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/components/${componentId}/update`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("update")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Pencil className="h-5 w-5 mr-2" />
                <span>Update</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/components/${componentId}/info`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("info")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Info className="h-5 w-5 mr-2" />
                <span>Info</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() => navigate(`/components/${componentId}/settings`)}
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("settings")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Settings className="h-5 w-5 mr-2" />
                <span>Settings</span>
              </Button>
            </li>
          </ul>
        </div>
      </nav>
    </ErrorBoundary>
  );
};

export default ComponentLeftNav;

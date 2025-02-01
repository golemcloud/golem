import { useNavigate, useParams, useLocation } from "react-router-dom";
import {
  Container,
  Home,
  Settings,
  ArrowLeft,
  Tv,
  Workflow,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import ErrorBoundary from "@/components/errorBoundary";

const WorkerLeftNav = () => {
  const navigate = useNavigate();
  const { componentId = "", workerName = "" } = useParams();
  const location = useLocation();

  const isActive = (path: string) => location.pathname.endsWith(path);

  return (
    <ErrorBoundary>
      <nav className="w-64 border-r p-4 border-gray-200 dark:border-gray-700 min-h-[93vh]">
        <div className="mb-6">
          <div className="flex items-center mb-6">
            <div onClick={() => navigate(`/components/${componentId}/workers`)}>
              <ArrowLeft className="h-5 w-5 mr-2 text-gray-800 dark:text-gray-200 hover:text-gray-600 dark:hover:text-gray-400 cursor-pointer" />
            </div>
            <h1 className="text-lg font-semibold text-gray-800 dark:text-gray-200">
              Worker
            </h1>
          </div>

          <ul className="space-y-1">
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(`/components/${componentId}/workers/${workerName}`)
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive(workerName ?? "")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Home className="h-5 w-5 mr-2" />
                <span>Overview</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(
                    `/components/${componentId}/workers/${workerName}/live`
                  )
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("live")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Tv className="h-5 w-5 mr-2" />
                <span>Live</span>
              </Button>
            </li>

            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(
                    `/components/${componentId}/workers/${workerName}/environments`
                  )
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("environment")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Container className="h-4 w-4 mr-2" />
                <span>Environment</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(
                    `/components/${componentId}/workers/${workerName}/invoke`
                  )
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("invoke")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Workflow className="h-4 w-4 mr-2" />
                <span>Invoke</span>
              </Button>
            </li>
            <li>
              <Button
                variant="ghost"
                onClick={() =>
                  navigate(
                    `/components/${componentId}/workers/${workerName}/manage`
                  )
                }
                className={cn(
                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                  isActive("manage")
                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                )}
              >
                <Settings className="h-5 w-5 mr-2" />
                <span>Manage</span>
              </Button>
            </li>
          </ul>
        </div>
      </nav>
    </ErrorBoundary>
  );
};

export default WorkerLeftNav;

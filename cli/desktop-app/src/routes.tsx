import ComponentInvoke from "@/pages/components/details/invoke.tsx";
import { Dashboard } from "@/pages/dashboard";
import FileManager from "@/pages/components/details/file.tsx";
import { RouteObject } from "react-router-dom";
import WorkerInfo from "@/pages/workers/details/info.tsx";
import WorkerInvoke from "@/pages/workers/details/invoke.tsx";
import { lazy } from "react";
import { Home } from "@/pages/home";
import AppLayout from "@/layouts/app-layout";
import CreateApplication from "@/pages/app-create";
import SettingsPage from "@/pages/settings";
import { ProfileSettingsPage } from "@/pages/settings/profiles";
import { CliPathSettingsPage } from "@/pages/settings/cli-path";
import { NotFoundPage } from "@/pages/not-found";

// Lazy load route components for code splitting and performance improvement
// Lazy-loading improves initial load times by loading components only when needed.
const Components = lazy(() => import("@/pages/components"));
const CreateComponent = lazy(() => import("@/pages/components/create"));
const APIs = lazy(() =>
  import("@/pages/api").then(module => ({ default: module.APIs })),
);
const CreateAPI = lazy(() => import("@/pages/api/create"));
const APIDetails = lazy(() => import("@/pages/api/details"));
const APISettings = lazy(() => import("@/pages/api/details/settings"));
const CreateRoute = lazy(() => import("@/pages/api/details/createRoute.tsx"));
const Deployments = lazy(() => import("@/pages/deployment"));
const ComponentDetails = lazy(() =>
  import("@/pages/components/details").then(module => ({
    default: module.ComponentDetails,
  })),
);
const PluginList = lazy(() => import("@/pages/plugin"));
const ComponentSettings = lazy(
  () => import("@/pages/components/details/settings"),
);
const ComponentInfo = lazy(() => import("@/pages/components/details/info"));
const Exports = lazy(() => import("@/pages/components/details/export"));
// const ComponentUpdate = lazy(() => import("@/pages/components/details/update"));
const WorkerList = lazy(() => import("@/pages/workers"));
const APINewVersion = lazy(() => import("@/pages/api/details/newVersion"));
const CreateWorker = lazy(() => import("@/pages/workers/create"));
const WorkerDetails = lazy(() => import("@/pages/workers/details"));
const WorkerEnvironments = lazy(
  () => import("@/pages/workers/details/environments"),
);
const WorkerManage = lazy(() => import("@/pages/workers/details/manage"));
const WorkerLive = lazy(() => import("@/pages/workers/details/live"));
const CreatePlugin = lazy(() => import("@/pages/plugin/create"));
const PluginView = lazy(() =>
  import("@/pages/plugin/view").then(module => ({
    default: module.PluginView,
  })),
);
const ApiRoute = lazy(() =>
  import("@/pages/api/details/viewRoute").then(module => ({
    default: module.ApiRoute,
  })),
);
const CreateDeployment = lazy(() => import("@/pages/deployment/create"));
const ApiLayout = lazy(() =>
  import("@/pages/api/details/apis-layout").then(module => ({
    default: module.ApiLayout,
  })),
);
const Plugins = lazy(() => import("@/pages/components/details/plugin"));
const ComponentLayout = lazy(() =>
  import("@/pages/components/details/component-layout").then(module => ({
    default: module.ComponentLayout,
  })),
);
const WorkerLayout = lazy(() =>
  import("@/pages/workers/details/worker-layout").then(module => ({
    default: module.WorkerLayout,
  })),
);

// Route configuration constants for ease of maintenance
export const ROUTES = {
  HOME: "",
  APP_CREATE: "/app-create",
  APP: "/app/:appId",
  DASHBOARD: "/app/:appId/dashboard",
  COMPONENTS: "/app/:appId/components",
  COMPONENTS_CREATE: "/app/:appId/components/create",
  COMPONENTS_DETAIL: "/app/:appId/components/:componentId",
  APIS: "/app/:appId/apis",
  APIS_CREATE: "/app/:appId/apis/create",
  APIS_DETAIL: "/app/:appId/apis/:apiName/version/:version",
  DEPLOYMENTS: "/app/:appId/deployments",
  DEPLOYMENTS_CREATE: "/app/:appId/deployments/create",
  PLUGINS: "/app/:appId/plugins",
  PLUGINS_CREATE: "/app/:appId/plugins/create",
  PLUGINS_DETAIL: "/app/:appId/plugins/:pluginId",
  PLUGINS_VERSION: "/app/:appId/plugins/:pluginId/:version",
};

export const appRoutes: RouteObject[] = [
  {
    path: ROUTES.HOME,
    element: <AppLayout />,
    children: [
      {
        path: "/",
        element: <Home />,
      },
      {
        path: "app-create",
        element: <CreateApplication />,
      },
      {
        path: "settings",
        element: <SettingsPage />,
        children: [
          {
            index: true,
            element: <ProfileSettingsPage />,
          },
          {
            path: "cli-path",
            element: <CliPathSettingsPage />,
          },
        ],
      },
    ],
  },
  {
    path: ROUTES.APP,
    element: <AppLayout />,
    children: [
      {
        path: "",
        element: <Dashboard />,
      },
      {
        path: "dashboard",
        element: <Dashboard />,
      },
      {
        path: "components",
        element: <Components />,
      },
      {
        path: "components/create",
        element: <CreateComponent />,
      },
      {
        path: "components/:componentId",
        element: <ComponentLayout />,
        children: [
          { path: "", element: <ComponentDetails /> },
          { path: "settings", element: <ComponentSettings /> },
          // { path: "update", element: <ComponentUpdate /> },
          { path: "info", element: <ComponentInfo /> },
          { path: "exports", element: <Exports /> },
          { path: "plugins", element: <Plugins /> },
          { path: "files", element: <FileManager /> },
          { path: "invoke", element: <ComponentInvoke /> },
          { path: "workers", element: <WorkerList /> },
          { path: "workers/create", element: <CreateWorker /> },
        ],
      },
      {
        path: "components/:componentId/workers/:workerName",
        element: <WorkerLayout />,
        children: [
          { path: "", element: <WorkerDetails /> },
          { path: "environments", element: <WorkerEnvironments /> },
          { path: "info", element: <WorkerInfo /> },
          { path: "manage", element: <WorkerManage /> },
          { path: "invoke", element: <WorkerInvoke /> },
          { path: "live", element: <WorkerLive /> },
        ],
      },
      {
        path: "apis",
        element: <APIs />,
      },
      {
        path: "apis/create",
        element: <CreateAPI />,
      },
      {
        path: "apis/:apiName/version/:version",
        element: <ApiLayout />,
        children: [
          { path: "", element: <APIDetails /> },
          { path: "settings", element: <APISettings /> },
          { path: "routes/add", element: <CreateRoute key="create" /> },
          { path: "routes/edit", element: <CreateRoute key="edit" /> },
          { path: "newversion", element: <APINewVersion /> },
          { path: "routes", element: <ApiRoute /> },
        ],
      },
      {
        path: "deployments",
        element: <Deployments />,
      },
      {
        path: "deployments/create",
        element: <CreateDeployment />,
      },
      {
        path: "plugins",
        element: <PluginList />,
      },
      {
        path: "plugins/create",
        element: <CreatePlugin />,
      },
      {
        path: "plugins/:pluginId",
        element: <PluginView />,
      },
      {
        path: "plugins/:pluginId/:version",
        element: <PluginView />,
      },
    ],
  },
  // Catch-all route for 404 pages
  {
    path: "*",
    element: <NotFoundPage />,
  },
];

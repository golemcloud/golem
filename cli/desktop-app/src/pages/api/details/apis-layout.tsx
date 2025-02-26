import { API } from "@/service";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Outlet,
  useLocation,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router-dom";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar.tsx";
import { SidebarMenu } from "@/components/sidebar.tsx";
import { Separator } from "@/components/ui/separator.tsx";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { CircleFadingPlusIcon, Home, Plus, Settings } from "lucide-react";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb.tsx";
import { Api } from "@/types/api.ts";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select.tsx";
import { Badge } from "@/components/ui/badge.tsx";
import { SelectValue } from "@radix-ui/react-select";
import YamlUploader from "@/components/yaml-uploader.tsx";
import { NavRoutes } from "@/components/nav-route.tsx";
import { Button } from "@/components/ui/button.tsx";

const MenuItems = (apiName: string, version: string) => [
  {
    title: "Overview",
    url: `/apis/${apiName}/version/${version}`,
    icon: Home,
  },
  {
    title: "Settings",
    url: `/apis/${apiName}/version/${version}/settings`,
    icon: Settings,
  },
  {
    title: "New Version",
    url: `/apis/${apiName}/version/${version}/newversion`,
    icon: CircleFadingPlusIcon,
  },
];

export const ApiLayout = () => {
  const { apiName, version } = useParams();
  const [queryParams] = useSearchParams();
  const navigate = useNavigate();
  const [apiDetails, setApiDetails] = useState([] as Api[]);

  const [currentApiDetails, setCurrentApiDetails] = useState({} as Api);
  const [currentMenu, setCurrentMenu] = useState("Overview");

  const basePath = useLocation().pathname.replace(
    `/apis/${apiName}/version/${version}`,
    "",
  );
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const reload = queryParams.get("reload");
  const sortedVersions = useMemo(() => {
    return [...apiDetails].sort((a, b) =>
      b.version.localeCompare(a.version, undefined, { numeric: true }),
    );
  }, [apiDetails]);

  useEffect(() => {
    API.getApi(apiName!).then(async response => {
      setApiDetails(response);
      const selectedApi = response.find(api => api.version === version);
      if (selectedApi) {
        setCurrentApiDetails(selectedApi);
      }
    });
    if (location.pathname.includes("settings")) setCurrentMenu("Settings");
    else if (location.pathname.includes("routes/add"))
      setCurrentMenu("Add New Route");
    else if (path) setCurrentMenu(path);
    else if (location.pathname.includes("newversion"))
      setCurrentMenu("New Version");
    else if (location.pathname.includes("manage")) setCurrentMenu("Manage");
  }, [apiName, version, path, method, reload]);

  const handleNavigateHome = useCallback(() => {
    navigate(`/apis/${apiName}/version/${version}`);
    setCurrentMenu("Overview");
  }, [navigate, apiName, version]);

  return (
    <ErrorBoundary>
      <SidebarProvider>
        <SidebarMenu
          menus={MenuItems(apiName!, version!)}
          activeItem={currentMenu}
          setActiveItem={setCurrentMenu}
          title={"Worker"}
        >
          {currentApiDetails?.routes?.length > 0 && (
            <NavRoutes
              routes={(currentApiDetails?.routes || []).map(route => {
                return {
                  method: route.method,
                  name: route.path,
                  url: `/apis/${apiName}/version/${version}/routes/?path=${route.path}&method=${route.method}`,
                };
              })}
              setActiveItem={value => setCurrentMenu(value)}
              activeItem={currentMenu}
            />
          )}
        </SidebarMenu>
        <SidebarInset>
          <header className="flex h-16 shrink-0 items-center gap-2 transition-[width,height] ease-linear group-has-[[data-collapsible=icon]]/sidebar-wrapper:h-12 border-b">
            <div className="flex items-center gap-2 px-4">
              <SidebarTrigger className="-ml-1" />
              <Separator orientation="vertical" className="mr-2 h-4" />

              <Breadcrumb>
                <BreadcrumbList>
                  <BreadcrumbItem className="hidden md:block cursor-pointer">
                    <BreadcrumbLink asChild>
                      <span onClick={handleNavigateHome}>{apiName}</span>
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator className="hidden md:block" />
                  <BreadcrumbItem>
                    <BreadcrumbPage>{currentMenu}</BreadcrumbPage>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
            </div>
            {/*push this to right*/}
            <div className={"flex items-center gap-4"}>
              <div className="flex items-center gap-2">
                <Select
                  defaultValue={version}
                  onValueChange={version => {
                    const selectedApi = apiDetails.find(
                      api => api.version === version,
                    );
                    if (selectedApi) {
                      navigate(
                        `/apis/${apiName}/version/${version}${basePath}`,
                      );
                    }
                  }}
                >
                  <SelectTrigger
                  // className={"rounded border-transparent bg-blue-950 text-warning-foreground px-2 py-0.5 m-0"}
                  >
                    <SelectValue>{version}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {sortedVersions.map(api => (
                      <SelectItem value={api.version} key={api.version}>
                        <div className="flex items-center gap-2">
                          <span className="text-sm">{api.version}</span>
                          {api.draft ? (
                            <Badge
                              variant="warning"
                              className="p-0.5 m-0 rounded"
                            >
                              Draft
                            </Badge>
                          ) : (
                            <Badge
                              variant="success"
                              className="p-0.5 m-0 rounded"
                            >
                              Published
                            </Badge>
                          )}
                        </div>
                        {/*{api.version} {api.draft ? "(Draft)" : "(Published)"}*/}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="flex items-center gap-2 ml-auto px-4">
              <YamlUploader />
              <Button
                variant="default"
                onClick={() => {
                  navigate(`/apis/${apiName}/version/${version}/routes/add?`);
                  setCurrentMenu("Add New Route");
                }}
              >
                <Plus className="h-5 w-5" />
                <span>Add</span>
              </Button>
            </div>
          </header>
          <ErrorBoundary>
            <Outlet />
          </ErrorBoundary>
        </SidebarInset>
      </SidebarProvider>
    </ErrorBoundary>
  );
};

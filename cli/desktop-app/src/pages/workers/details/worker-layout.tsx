import { API } from "@/service";
import { useEffect, useState } from "react";
import { Outlet, useLocation, useNavigate, useParams } from "react-router-dom";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar.tsx";
import { SidebarMenu } from "@/components/sidebar.tsx";
import { Separator } from "@/components/ui/separator.tsx";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { ComponentList } from "@/types/component.ts";
import { Container, Home, Info, Settings, Tv, Workflow } from "lucide-react";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb.tsx";

const MenuItems = (appId: string, componentId: string, workerName: string) => [
  {
    title: "Overview",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}`,
    icon: Home,
  },
  {
    title: "Live",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}/live`,
    icon: Tv,
  },
  {
    title: "Environment",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}/environments`,
    icon: Container,
  },
  {
    title: "Invoke",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}/invoke`,
    icon: Workflow,
  },
  {
    title: "Info",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}/info`,
    icon: Info,
  },
  {
    title: "Manage",
    url: `/app/${appId}/components/${componentId}/workers/${workerName}/manage`,
    icon: Settings,
  },
];

export const WorkerLayout = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { componentId = "", workerName = "", appId } = useParams();
  const [currentComponent, setCurrentComponent] = useState({} as ComponentList);
  const [currentMenu, setCurrentMenu] = useState("Overview");

  useEffect(() => {
    if (componentId) {
      API.componentService.getComponentByIdAsKey(appId!).then(response => {
        setCurrentComponent(response[componentId]!);
      });
    }
  }, [componentId]);

  useEffect(() => {
    if (location.pathname.includes("live")) setCurrentMenu("Live");
    else if (location.pathname.includes("environments"))
      setCurrentMenu("Environment");
    else if (location.pathname.includes("invoke")) setCurrentMenu("Invoke");
    else if (location.pathname.includes("manage")) setCurrentMenu("Manage");
    else if (location.pathname.includes("info")) setCurrentMenu("Info");
  }, [location.pathname]);

  const navigateHome = () => {
    navigate(`/app/${appId}/components/${componentId}/workers/${workerName}`);
    setCurrentMenu("Overview");
  };

  return (
    <ErrorBoundary>
      <SidebarProvider>
        <SidebarMenu
          menus={MenuItems(appId!, componentId, workerName)}
          activeItem={currentMenu}
          setActiveItem={setCurrentMenu}
          title={"Worker"}
        />
        <SidebarInset>
          <header className="flex h-16 shrink-0 items-center gap-2 transition-[width,height] ease-linear group-has-[[data-collapsible=icon]]/sidebar-wrapper:h-12 border-b">
            <div className="flex items-center gap-2 px-4">
              <SidebarTrigger className="-ml-1" />
              <Separator orientation="vertical" className="mr-2 h-4" />

              <Breadcrumb>
                <BreadcrumbList>
                  <BreadcrumbItem className="hidden md:block cursor-pointer">
                    <BreadcrumbLink asChild>
                      <span
                        onClick={() =>
                          navigate(`/app/${appId}/components/${componentId}`)
                        }
                      >
                        <span className="text-gray-500">Component:</span>{" "}
                        {currentComponent.componentName}
                      </span>
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator className="hidden md:block" />
                  <BreadcrumbItem>
                    <BreadcrumbLink asChild>
                      <span
                        onClick={() => navigateHome()}
                        className="cursor-pointer"
                      >
                        <span className="text-gray-500">Worker:</span>{" "}
                        {workerName}
                      </span>
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator className="hidden md:block" />
                  <BreadcrumbItem>
                    <BreadcrumbPage>{currentMenu}</BreadcrumbPage>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
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

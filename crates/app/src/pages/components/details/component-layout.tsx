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
import {
  ArrowRightFromLine,
  Home,
  Info,
  Pencil,
  Pickaxe,
  Settings,
  ToyBrick,
  Workflow,
} from "lucide-react";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb.tsx";

const MenuItems = (componentId: string, componentType: string) => [
  {
    title: "Overview",
    url: `/components/${componentId}`,
    icon: Home,
  },
  {
    title: "Workers",
    url: `/components/${componentId}/workers`,
    icon: Pickaxe,
    isHidden: componentType === "Ephemeral",
  },
  {
    title: "Invoke",
    url: `/components/${componentId}/invoke`,
    icon: Workflow,
    isHidden: componentType === "Durable",
  },
  {
    title: "Exports",
    url: `/components/${componentId}/exports`,
    icon: ArrowRightFromLine,
  },
  {
    title: "Update",
    url: `/components/${componentId}/update`,
    icon: Pencil,
  },
  {
    title: "Plugins",
    url: `/components/${componentId}/plugins`,
    icon: ToyBrick,
  },
  {
    title: "Info",
    url: `/components/${componentId}/info`,
    icon: Info,
  },
  {
    title: "Settings",
    url: `/components/${componentId}/settings`,
    icon: Settings,
  },
];

export const ComponentLayout = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { componentId = "" } = useParams();
  const [currentComponent, setCurrentComponent] = useState({} as ComponentList);
  const [currentMenu, setCurrentMenu] = useState("Overview");

  useEffect(() => {
    if (componentId) {
      API.getComponentByIdAsKey().then((response) => {
        setCurrentComponent(response[componentId]);
      });
    }
  }, [componentId]);

  useEffect(() => {
    if (location.pathname.includes("workers")) setCurrentMenu("Workers");
    else if (location.pathname.includes("invoke")) setCurrentMenu("Invoke");
    else if (location.pathname.includes("exports")) setCurrentMenu("Exports");
    else if (location.pathname.includes("update")) setCurrentMenu("Update");
    else if (location.pathname.includes("plugins")) setCurrentMenu("Plugins");
    else if (location.pathname.includes("info")) setCurrentMenu("Info");
    else if (location.pathname.includes("settings")) setCurrentMenu("Settings");
  }, [location.pathname]);

  const navigateHome = () => {
    navigate(`/components/${componentId}`);
    setCurrentMenu("Overview");
  };

  return (
    <ErrorBoundary>
      <SidebarProvider>
        <SidebarMenu
          menus={MenuItems(componentId, currentComponent.componentType!)}
          activeItem={currentMenu}
          setActiveItem={setCurrentMenu}
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
                      <span onClick={() => navigateHome()}>
                        {currentComponent.componentName}
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

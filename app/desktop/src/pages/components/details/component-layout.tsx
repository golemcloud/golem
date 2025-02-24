import { API } from "@/service";
import { useCallback, useEffect, useMemo, useState } from "react";
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
  Folder,
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
import { SidebarMenuProps } from "@/components/nav-main.tsx";

/**
 * Creates menu items for the component sidebar
 */
const createMenuItems = (
  componentId: string,
  componentType: string,
): SidebarMenuProps[] => [
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
    title: "Files",
    url: `/components/${componentId}/files`,
    icon: Folder,
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
    isHidden: componentType === "Ephemeral",
  },
];

/**
 * Layout component for the component details page
 */
export const ComponentLayout = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { componentId = "" } = useParams();
  const [currentComponent, setCurrentComponent] =
    useState<ComponentList | null>(null);
  const [currentMenu, setCurrentMenu] = useState("Overview");

  // Fetch component data
  const fetchComponent = useCallback(async () => {
    if (componentId) {
      try {
        const response = await API.getComponentByIdAsKey();
        setCurrentComponent(response[componentId]);
      } catch (error) {
        console.error("Error fetching component:", error);
      }
    }
  }, [componentId]);

  useEffect(() => {
    fetchComponent();
  }, [fetchComponent]);

  // Update current menu based on location
  useEffect(() => {
    if (location.pathname.includes("workers")) setCurrentMenu("Workers");
    else if (location.pathname.includes("invoke")) setCurrentMenu("Invoke");
    else if (location.pathname.includes("exports")) setCurrentMenu("Exports");
    else if (location.pathname.includes("update")) setCurrentMenu("Update");
    else if (location.pathname.includes("plugins")) setCurrentMenu("Plugins");
    else if (location.pathname.includes("info")) setCurrentMenu("Info");
    else if (location.pathname.includes("settings")) setCurrentMenu("Settings");
    else if (location.pathname.includes("files")) setCurrentMenu("Files");
    else setCurrentMenu("Overview");
  }, [location.pathname]);

  // Memoize menu items
  const menuItems = useMemo(() => {
    return createMenuItems(componentId, currentComponent?.componentType || "");
  }, [componentId, currentComponent?.componentType]);

  const handleNavigateHome = useCallback(() => {
    navigate(`/components/${componentId}`);
    setCurrentMenu("Overview");
  }, [navigate, componentId]);

  // Memoize header component
  const Header = useMemo(
    () => (
      <header className="flex h-16 shrink-0 items-center gap-2 transition-[width,height] ease-linear group-has-[[data-collapsible=icon]]/sidebar-wrapper:h-12 border-b">
        <div className="flex items-center gap-2 px-4">
          <SidebarTrigger className="-ml-1" />
          <Separator orientation="vertical" className="mr-2 h-4" />

          <Breadcrumb>
            <BreadcrumbList>
              <BreadcrumbItem className="hidden md:block cursor-pointer">
                <BreadcrumbLink asChild>
                  <span onClick={handleNavigateHome}>
                    <span className="text-gray-500">Component:</span>{" "}
                    {currentComponent?.componentName || "Loading..."}
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
    ),
    [currentComponent?.componentName, currentMenu, handleNavigateHome],
  );

  if (!currentComponent) {
    return <div>Loading...</div>;
  }

  return (
    <ErrorBoundary>
      <SidebarProvider>
        <SidebarMenu
          menus={menuItems}
          activeItem={currentMenu}
          setActiveItem={setCurrentMenu}
          title={"Component"}
        />
        <SidebarInset>
          {Header}
          <ErrorBoundary>
            <Outlet />
          </ErrorBoundary>
        </SidebarInset>
      </SidebarProvider>
    </ErrorBoundary>
  );
};

export default ComponentLayout;

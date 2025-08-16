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
  Pickaxe,
  Settings,
  ToyBrick,
  Workflow,
  Play,
  RefreshCw,
  Upload,
  Trash2,
  FileText,
  Send,
  Loader2,
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
import { Button } from "@/components/ui/button.tsx";
import { toast } from "@/hooks/use-toast";
import { YamlViewerModal } from "@/components/yaml-viewer-modal";
import { useLogViewer } from "@/contexts/log-viewer-context";

/**
 * Creates menu items for the component sidebar
 */
const createMenuItems = (
  appId: string,
  componentId: string,
  componentType: string,
): SidebarMenuProps[] => [
  {
    title: "Overview",
    url: `/app/${appId}/components/${componentId}`,
    icon: Home,
  },
  {
    title: "Workers",
    url: `/app/${appId}/components/${componentId}/workers`,
    icon: Pickaxe,
    isHidden: componentType === "Ephemeral",
  },
  {
    title: "Invoke",
    url: `/app/${appId}/components/${componentId}/invoke`,
    icon: Workflow,
    isHidden: componentType === "Durable",
  },
  {
    title: "Exports",
    url: `/app/${appId}/components/${componentId}/exports`,
    icon: ArrowRightFromLine,
  },
  // {
  //   title: "Update",
  //   url: `/app/${appId}/components/${componentId}/update`,
  //   icon: Pencil,
  // },
  {
    title: "Files",
    url: `/app/${appId}/components/${componentId}/files`,
    icon: Folder,
  },
  {
    title: "Plugins",
    url: `/app/${appId}/components/${componentId}/plugins`,
    icon: ToyBrick,
  },
  {
    title: "Info",
    url: `/app/${appId}/components/${componentId}/info`,
    icon: Info,
  },
  {
    title: "Settings",
    url: `/app/${appId}/components/${componentId}/settings`,
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
  const { componentId = "", appId } = useParams();
  const { showLog } = useLogViewer();
  const [currentComponent, setCurrentComponent] =
    useState<ComponentList | null>(null);
  const [currentMenu, setCurrentMenu] = useState("Overview");
  const [loadingStates, setLoadingStates] = useState({
    build: false,
    updateWorkers: false,
    deployWorkers: false,
    deployComponent: false,
    clean: false,
  });
  const [isYamlModalOpen, setIsYamlModalOpen] = useState(false);
  const [yamlContent, setYamlContent] = useState<string>("");

  // Component-level action handlers
  const handleBuildComponent = () => {
    if (!appId || !currentComponent?.componentName) return;
    setLoadingStates(prev => ({ ...prev, build: true }));

    // Run async operation without blocking using .then()
    API.appService
      .buildApp(appId, [currentComponent.componentName])
      .then(result => {
        if (result.success) {
          toast({
            title: "Build Completed",
            description: `Component ${currentComponent.componentName} build completed successfully.`,
          });
        } else {
          showLog({
            title: "Component Build Failed",
            logs: result.logs,
            status: "error",
            operation: `Build ${currentComponent.componentName}`,
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Component Build Failed",
          logs: String(error),
          status: "error",
          operation: `Build ${currentComponent.componentName}`,
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, build: false }));
      });
  };

  const handleUpdateWorkers = () => {
    if (!appId || !currentComponent?.componentName) return;
    setLoadingStates(prev => ({ ...prev, updateWorkers: true }));

    // Run async operation without blocking using .then()
    API.appService
      .updateWorkers(appId, [currentComponent.componentName])
      .then(result => {
        if (result.success) {
          toast({
            title: "Workers Update Completed",
            description: `Workers for ${currentComponent.componentName} updated successfully.`,
          });
        } else {
          showLog({
            title: "Workers Update Failed",
            logs: result.logs,
            status: "error",
            operation: `Update Workers for ${currentComponent.componentName}`,
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Workers Update Failed",
          logs: String(error),
          status: "error",
          operation: `Update Workers for ${currentComponent.componentName}`,
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, updateWorkers: false }));
      });
  };

  const handleDeployWorkers = () => {
    if (!appId || !currentComponent?.componentName) return;
    setLoadingStates(prev => ({ ...prev, deployWorkers: true }));

    // Run async operation without blocking using .then()
    API.appService
      .deployWorkers(appId, [currentComponent.componentName])
      .then(result => {
        if (result.success) {
          toast({
            title: "Deployment Completed",
            description: `Workers for ${currentComponent.componentName} deployed successfully.`,
          });
        } else {
          showLog({
            title: "Deployment Failed",
            logs: result.logs,
            status: "error",
            operation: `Deploy Workers for ${currentComponent.componentName}`,
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Deployment Failed",
          logs: String(error),
          status: "error",
          operation: `Deploy Workers for ${currentComponent.componentName}`,
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, deployWorkers: false }));
      });
  };

  const handleCleanComponent = () => {
    if (!appId || !currentComponent?.componentName) return;
    setLoadingStates(prev => ({ ...prev, clean: true }));

    // Run async operation without blocking using .then()
    API.appService
      .cleanApp(appId, [currentComponent.componentName])
      .then(result => {
        if (result.success) {
          toast({
            title: "Clean Completed",
            description: `Component ${currentComponent.componentName} cleaned successfully.`,
          });
        } else {
          showLog({
            title: "Component Clean Failed",
            logs: result.logs,
            status: "error",
            operation: `Clean ${currentComponent.componentName}`,
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Component Clean Failed",
          logs: String(error),
          status: "error",
          operation: `Clean ${currentComponent.componentName}`,
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, clean: false }));
      });
  };

  const handleViewComponentYaml = async () => {
    if (!appId || !currentComponent?.componentName) return;
    try {
      const yamlContent = await API.manifestService.getComponentYamlContent(
        appId,
        currentComponent.componentName,
      );
      setYamlContent(yamlContent);
      setIsYamlModalOpen(true);
    } catch (error) {
      toast({
        title: "Failed to Load Component YAML",
        description: String(error),
        variant: "destructive",
      });
    }
  };

  const handleDeployComponent = () => {
    if (!appId || !currentComponent?.componentName) return;
    setLoadingStates(prev => ({ ...prev, deployComponent: true }));

    // Run async operation without blocking using .then()
    API.appService
      .deployWorkers(appId, [currentComponent.componentName])
      .then(result => {
        if (result.success) {
          toast({
            title: "Component Deployment Completed",
            description: `Component ${currentComponent.componentName} deployed successfully.`,
          });
        } else {
          showLog({
            title: "Component Deployment Failed",
            logs: result.logs,
            status: "error",
            operation: `Deploy ${currentComponent.componentName}`,
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Component Deployment Failed",
          logs: String(error),
          status: "error",
          operation: `Deploy ${currentComponent.componentName}`,
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, deployComponent: false }));
      });
  };

  // Fetch component data
  const fetchComponent = useCallback(async () => {
    if (componentId) {
      try {
        const response = await API.componentService.getComponentByIdAsKey(
          appId!,
        );
        setCurrentComponent(response[componentId]!);
      } catch (error) {
        console.error("Error fetching component:", error);
      }
    }
  }, [componentId, appId]);

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
    return createMenuItems(
      appId!,
      componentId,
      currentComponent?.componentType || "",
    );
  }, [componentId, currentComponent?.componentType, appId]);

  const handleNavigateHome = useCallback(() => {
    navigate(`/app/${appId}/components/${componentId}`);
    setCurrentMenu("Overview");
  }, [navigate, componentId, appId]);

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

        {/* Component Actions */}
        <div className="flex items-center gap-1 ml-auto px-4">
          <Button
            variant="outline"
            size="sm"
            onClick={handleBuildComponent}
            disabled={loadingStates.build}
            className="text-xs h-7"
          >
            {loadingStates.build ? (
              <Loader2 className="h-3 w-3 mr-1 animate-spin" />
            ) : (
              <Play className="h-3 w-3 mr-1" />
            )}
            Build
          </Button>
          {currentComponent?.componentType === "Durable" && (
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={handleUpdateWorkers}
                disabled={loadingStates.updateWorkers}
                className="text-xs h-7"
              >
                {loadingStates.updateWorkers ? (
                  <Loader2 className="h-3 w-3 mr-1 animate-spin" />
                ) : (
                  <RefreshCw className="h-3 w-3 mr-1" />
                )}
                Update
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={handleDeployWorkers}
                disabled={loadingStates.deployWorkers}
                className="text-xs h-7"
              >
                {loadingStates.deployWorkers ? (
                  <Loader2 className="h-3 w-3 mr-1 animate-spin" />
                ) : (
                  <Upload className="h-3 w-3 mr-1" />
                )}
                Deploy
              </Button>
            </>
          )}
          <Button
            variant="outline"
            size="sm"
            onClick={handleDeployComponent}
            disabled={loadingStates.deployComponent}
            className="text-xs h-7"
          >
            {loadingStates.deployComponent ? (
              <Loader2 className="h-3 w-3 mr-1 animate-spin" />
            ) : (
              <Send className="h-3 w-3 mr-1" />
            )}
            Deploy Component
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleCleanComponent}
            disabled={loadingStates.clean}
            className="text-xs h-7"
          >
            {loadingStates.clean ? (
              <Loader2 className="h-3 w-3 mr-1 animate-spin" />
            ) : (
              <Trash2 className="h-3 w-3 mr-1" />
            )}
            Clean
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleViewComponentYaml}
            className="text-xs h-7"
          >
            <FileText className="h-3 w-3 mr-1" />
            YAML
          </Button>
        </div>
      </header>
    ),
    [
      currentComponent?.componentName,
      currentComponent?.componentType,
      currentMenu,
      handleNavigateHome,
      loadingStates,
    ],
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

      {/* YAML Viewer Modal */}
      <YamlViewerModal
        isOpen={isYamlModalOpen}
        onOpenChange={setIsYamlModalOpen}
        title={`Component Manifest (${currentComponent?.componentName || "golem"}.yaml)`}
        yamlContent={yamlContent}
        appId={appId}
        componentId={componentId}
        isAppYaml={false}
      />
    </ErrorBoundary>
  );
};

export default ComponentLayout;

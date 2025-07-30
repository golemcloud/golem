import {
  ComponentsSection,
  ComponentsSectionRef,
} from "@/pages/dashboard/componentSection.tsx";
import { APISection } from "@/pages/dashboard/apiSection.tsx";
import { DeploymentSection } from "@/pages/dashboard/deploymentSection.tsx";
import { useEffect, useState, useRef } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { storeService } from "@/lib/settings.ts";
import { API } from "@/service";
import { toast } from "@/hooks/use-toast";
import {
  Play,
  RefreshCw,
  Upload,
  Trash2,
  FileText,
  Send,
  Loader2,
} from "lucide-react";
import { YamlViewerModal } from "@/components/yaml-viewer-modal";
import { useLogViewer } from "@/contexts/log-viewer-context";

export const Dashboard = () => {
  const { appId } = useParams();
  const navigate = useNavigate();
  const { showLog } = useLogViewer();
  const componentsSectionRef = useRef<ComponentsSectionRef>(null);
  const [appName, setAppName] = useState<string>("");
  const [loadingStates, setLoadingStates] = useState({
    build: false,
    updateWorkers: false,
    deployWorkers: false,
    deployApp: false,
    clean: false,
  });
  const [isYamlModalOpen, setIsYamlModalOpen] = useState(false);
  const [yamlContent, setYamlContent] = useState<string>("");

  useEffect(() => {
    // If no app ID is in the URL, redirect to home
    if (!appId) {
      navigate("/");
    } else {
      // Get app name from storeService
      storeService.getAppById(appId).then(app => {
        if (app && app.name) {
          setAppName(app.name);
        }
      });
      (async () => {
        await storeService.updateAppLastOpened(appId);
      })();
    }
  }, [appId, navigate]);

  const handleBuildApp = () => {
    if (!appId) return;
    setLoadingStates(prev => ({ ...prev, build: true }));

    // Run async operation without blocking using .then()
    API.appService
      .buildApp(appId)
      .then(result => {
        if (result.success) {
          toast({
            title: "Build Completed",
            description: "Application build completed successfully.",
          });
        } else {
          showLog({
            title: "Build Failed",
            logs: result.logs,
            status: "error",
            operation: "Build App",
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Build Failed",
          logs: String(error),
          status: "error",
          operation: "Build App",
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, build: false }));
      });
  };

  const handleUpdateWorkers = () => {
    if (!appId) return;
    setLoadingStates(prev => ({ ...prev, updateWorkers: true }));

    // Run async operation without blocking using .then()
    API.appService
      .updateWorkers(appId)
      .then(result => {
        if (result.success) {
          toast({
            title: "Workers Update Completed",
            description: "Worker update process completed successfully.",
          });
        } else {
          showLog({
            title: "Workers Update Failed",
            logs: result.logs,
            status: "error",
            operation: "Update Workers",
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Workers Update Failed",
          logs: String(error),
          status: "error",
          operation: "Update Workers",
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, updateWorkers: false }));
      });
  };

  const handleDeployWorkers = () => {
    if (!appId) return;
    setLoadingStates(prev => ({ ...prev, deployWorkers: true }));

    // Run async operation without blocking using .then()
    API.appService
      .deployWorkers(appId)
      .then(result => {
        if (result.success) {
          toast({
            title: "Deployment Completed",
            description: "Worker deployment completed successfully.",
          });
        } else {
          showLog({
            title: "Deployment Failed",
            logs: result.logs,
            status: "error",
            operation: "Deploy Workers",
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Deployment Failed",
          logs: String(error),
          status: "error",
          operation: "Deploy Workers",
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, deployWorkers: false }));
      });
  };

  const handleCleanApp = () => {
    if (!appId) return;
    setLoadingStates(prev => ({ ...prev, clean: true }));

    // Run async operation without blocking using .then()
    API.appService
      .cleanApp(appId)
      .then(result => {
        if (result.success) {
          toast({
            title: "Clean Completed",
            description: "Application clean process completed successfully.",
          });
        } else {
          showLog({
            title: "Clean Failed",
            logs: result.logs,
            status: "error",
            operation: "Clean App",
          });
        }
      })
      .catch(error => {
        showLog({
          title: "Clean Failed",
          logs: String(error),
          status: "error",
          operation: "Clean App",
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, clean: false }));
      });
  };

  const handleDeployApp = () => {
    if (!appId) return;
    setLoadingStates(prev => ({ ...prev, deployApp: true }));

    // Run async operation without blocking using .then()
    API.appService
      .deployWorkers(appId)
      .then(result => {
        if (result.success) {
          toast({
            title: "Deploy Completed",
            description: "Application deployment completed successfully.",
          });
          // Refresh components list after successful deployment
          return componentsSectionRef.current?.refreshComponents();
        } else {
          showLog({
            title: "Deploy Failed",
            logs: result.logs,
            status: "error",
            operation: "Deploy App",
          });
          return Promise.reject(result.logs);
        }
      })
      .catch(error => {
        showLog({
          title: "Deploy Failed",
          logs: String(error),
          status: "error",
          operation: "Deploy App",
        });
      })
      .finally(() => {
        setLoadingStates(prev => ({ ...prev, deployApp: false }));
      });
  };

  const handleViewYaml = async () => {
    if (!appId) return;
    try {
      const yamlContent = await API.manifestService.getAppYamlContent(appId);
      setYamlContent(yamlContent);
      setIsYamlModalOpen(true);
    } catch (error) {
      toast({
        title: "Failed to Load YAML",
        description: String(error),
        variant: "destructive",
      });
    }
  };

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex justify-between items-center mb-6">
        <h1 className="text-3xl font-bold">Working in {appName || "App"}</h1>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => navigate("/")}>
            Back to Apps
          </Button>
        </div>
      </div>

      {/* App Actions Section */}
      <div className="bg-muted/20 border rounded-lg p-4 mb-6">
        <h2 className="text-lg font-semibold mb-3">App Actions</h2>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleBuildApp}
            disabled={loadingStates.build}
          >
            {loadingStates.build ? (
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            ) : (
              <Play className="h-4 w-4 mr-2" />
            )}
            Build App
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleUpdateWorkers}
            disabled={loadingStates.updateWorkers}
          >
            {loadingStates.updateWorkers ? (
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4 mr-2" />
            )}
            Update Workers
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleDeployWorkers}
            disabled={loadingStates.deployWorkers}
          >
            {loadingStates.deployWorkers ? (
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            ) : (
              <Upload className="h-4 w-4 mr-2" />
            )}
            Deploy Workers
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleDeployApp}
            disabled={loadingStates.deployApp}
          >
            {loadingStates.deployApp ? (
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            ) : (
              <Send className="h-4 w-4 mr-2" />
            )}
            Deploy App
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleCleanApp}
            disabled={loadingStates.clean}
          >
            {loadingStates.clean ? (
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            ) : (
              <Trash2 className="h-4 w-4 mr-2" />
            )}
            Clean App
          </Button>
          <Button variant="outline" size="sm" onClick={handleViewYaml}>
            <FileText className="h-4 w-4 mr-2" />
            View YAML
          </Button>
        </div>
      </div>

      <div className="grid flex-1 grid-cols-1 gap-4 lg:grid-cols-3 lg:gap-6 min-h-[85vh] mb-8">
        <ComponentsSection ref={componentsSectionRef} />
        <div className="grid grid-cols-1 gap-4 flex-col">
          <DeploymentSection />
          <APISection />
        </div>
      </div>

      {/* YAML Viewer Modal */}
      <YamlViewerModal
        isOpen={isYamlModalOpen}
        onOpenChange={setIsYamlModalOpen}
        title="Application Manifest (golem.yaml)"
        yamlContent={yamlContent}
        appId={appId}
        isAppYaml={true}
      />
    </div>
  );
};

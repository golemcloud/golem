import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Hammer, Upload, Loader2 } from "lucide-react";
import { useState } from "react";
import { API } from "@/service";
import { toast } from "@/hooks/use-toast";

interface UnbuiltComponentCardProps {
  name: string;
  appId: string;
  onBuildComplete?: () => void;
}

export const UnbuiltComponentCard = ({
  name,
  appId,
  onBuildComplete,
}: UnbuiltComponentCardProps) => {
  const [isBuildingState, setIsBuildingState] = useState(false);
  const [isDeployingState, setIsDeployingState] = useState(false);

  const handleBuild = async () => {
    setIsBuildingState(true);
    try {
      await API.cliService.callCLI(appId, "build", [name]);
      toast({
        title: "Build Successful",
        description: `Component ${name} has been built successfully.`,
      });
      // Refresh the component list
      onBuildComplete?.();
    } catch (error) {
      console.error(error);
    } finally {
      setIsBuildingState(false);
    }
  };

  const handleDeploy = async () => {
    setIsDeployingState(true);
    try {
      await API.cliService.callCLI(appId, "deploy", []);
      toast({
        title: "Deploy Successful",
        description: `Component ${name} has been deployed successfully.`,
      });
      // Refresh the component list
      onBuildComplete?.();
    } catch (error) {
      console.error(error);
    } finally {
      setIsDeployingState(false);
    }
  };

  return (
    <Card className="relative overflow-hidden bg-muted/30 dark:bg-muted/20 border-dashed">
      <CardHeader className="pb-3">
        <CardTitle className="text-lg font-semibold flex items-center justify-between">
          <span className="truncate">{name}</span>
          <span className="text-xs text-muted-foreground bg-muted px-2 py-1 rounded">
            WIP
          </span>
        </CardTitle>
      </CardHeader>
      <CardContent className="pb-4">
        <div className="text-sm text-muted-foreground mb-3">
          Waiting for deployment
        </div>

        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={handleBuild}
            disabled={isBuildingState || isDeployingState}
          >
            {isBuildingState ? (
              <Loader2 className="h-4 w-4 mr-1 animate-spin" />
            ) : (
              <Hammer className="h-4 w-4 mr-1" />
            )}
            Build
          </Button>

          <Button
            size="sm"
            variant="outline"
            onClick={handleDeploy}
            disabled={isBuildingState || isDeployingState}
          >
            {isDeployingState ? (
              <Loader2 className="h-4 w-4 mr-1 animate-spin" />
            ) : (
              <Upload className="h-4 w-4 mr-1" />
            )}
            Deploy
          </Button>
        </div>
      </CardContent>
    </Card>
  );
};

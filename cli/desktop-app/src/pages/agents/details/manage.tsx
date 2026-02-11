import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label.tsx";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.tsx";
import { toast } from "@/hooks/use-toast";
import { API } from "@/service";
import { ComponentList } from "@/types/component.ts";
import { Agent } from "@/types/agent.ts";
import { CircleFadingArrowUp, Pause, Play, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

export default function AgentManage() {
  const { appId, componentId = "", agentName = "" } = useParams();
  const navigate = useNavigate();
  const [agentDetails, setAgentDetails] = useState({} as Agent);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [showAgentUpgrade, setShowAgentUpgrade] = useState(false);
  const [upgradeTo, setUpgradeTo] = useState("0");
  const [upgradeType, setUpgradeType] = useState("Automatic");
  const [componentList, setComponentList] = useState<ComponentList>({});

  useEffect(() => {
    if (componentId && agentName) {
      API.componentService.getComponentByIdAsKey(appId!).then(response => {
        const componentListArray = Object.values(
          response as Record<string, ComponentList>,
        );
        const foundComponent = componentListArray.find(
          (x: ComponentList) => x.componentId === componentId,
        );
        if (foundComponent) {
          setComponentList(foundComponent);
        }
      });
      API.agentService
        .getParticularAgent(appId!, componentId, agentName)
        .then(response => {
          const agent = response.metadata;
          setAgentDetails(agent);
          setUpgradeTo(`${agent?.componentRevision}`);
        });
    }
  }, [componentId, agentName]);

  const handleUpgrade = () => {
    API.agentService
      .upgradeAgent(
        appId!,
        componentList.componentName!,
        agentDetails?.workerName!,
        Number(upgradeTo),
        upgradeType,
      )
      .then(() => {
        toast({
          title: "Agent upgraded Initiated",
          duration: 3000,
        });
      });
  };

  const handleDelete = () => {
    API.agentService.deleteAgent(appId!, componentId, agentName).then(() => {
      toast({
        title: "Agent deleted successfully",
        duration: 3000,
        variant: "destructive",
      });
      navigate(`/app/${appId}/components/${componentId}`);
    });
  };

  const onResumeAgent = () => {
    API.agentService.resumeAgent(appId!, componentId, agentName).then(() => {
      toast({
        title: "Agent resumed",
        duration: 3000,
      });
    });
  };

  const onInterruptAgent = () => {
    API.agentService.interruptAgent(appId!, componentId, agentName).then(() => {
      toast({
        title: "Agent interrupted",
        duration: 3000,
      });
    });
  };

  const versionListGreaterThan =
    componentList.versionList?.filter(
      version => version > agentDetails?.componentRevision,
    ) || [];

  return (
    <div className="flex flex-col items-center p-6 space-y-6 w-full max-w-4xl mx-auto">
      <Card className="w-full shadow-lg rounded-xl border border-border/30">
        <CardHeader>
          <CardTitle>Agent Execution</CardTitle>
          <CardDescription>Manage the agent and its execution.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Upgrade Agent</h3>
              <p className="text-sm text-muted-foreground">
                Upgrade Agent With New Component Version
              </p>
            </div>
            <Button
              variant="default"
              onClick={() => setShowAgentUpgrade(true)}
              className="bg-blue-600 text-white hover:bg-blue-700"
            >
              <CircleFadingArrowUp className="mr-2 h-4 w-4" />
              Upgrade Agent
            </Button>
          </div>
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Interrupt Agent</h3>
              <p className="text-sm text-muted-foreground">
                Interrupts the execution of a running agent
              </p>
            </div>
            <Button variant="secondary" onClick={onInterruptAgent}>
              <Pause className="mr-2 h-4 w-4" />
              Interrupt Agent
            </Button>
          </div>
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Resume Agent</h3>
              <p className="text-sm text-muted-foreground">
                Resumes the execution of an interrupted agent
              </p>
            </div>
            <Button variant="secondary" onClick={onResumeAgent}>
              <Play className="mr-2 h-4 w-4" />
              Resume Agent
            </Button>
          </div>
        </CardContent>
      </Card>
      <Card className="w-full border border-red-500/30 bg-red-50 dark:bg-red-900/10">
        <CardHeader>
          <CardTitle className="text-red-500">Danger Zone</CardTitle>
          <CardDescription>Proceed with caution.</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-lg font-semibold">Delete this Agent</h3>
              <p className="text-sm text-muted-foreground">
                Once you delete a agent, there is no going back. Please be
                certain.
              </p>
            </div>
            <Button
              variant="destructive"
              onClick={() => setShowDeleteDialog(true)}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete Agent
            </Button>
          </div>
        </CardContent>
      </Card>
      <AlertDialog open={showAgentUpgrade} onOpenChange={setShowAgentUpgrade}>
        <AlertDialogContent className="sm:max-w-[600px]">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-xl font-semibold">
              Upgrade Agent
            </AlertDialogTitle>
            <AlertDialogDescription className="text-sm text-muted-foreground">
              This action cannot be undone. This will permanently upgrade the
              agent to the selected version.
            </AlertDialogDescription>
          </AlertDialogHeader>

          <div className="grid gap-6 py-4">
            {/* Component ID */}
            <div className="grid grid-cols-4 items-center gap-4">
              <Label
                htmlFor="componentId"
                className="text-right text-sm font-medium"
              >
                Component Name
              </Label>
              <Input
                defaultValue={agentDetails.componentName || "N/A"}
                className="col-span-3 bg-muted/50"
                disabled
              />
            </div>

            {/* Current Component Version */}
            <div className="grid grid-cols-4 items-center gap-4">
              <Label
                htmlFor="componentRevision"
                className="text-right text-sm font-medium"
              >
                Current Version
              </Label>
              <Input
                defaultValue={agentDetails?.componentRevision}
                className="col-span-3 bg-muted/50"
                disabled
              />
            </div>

            {/* Upgrade Type (Automatic/Manual) */}
            <div className="grid grid-cols-4 items-center gap-4">
              <Label
                htmlFor="upgradeType"
                className="text-right text-sm font-medium"
              >
                Upgrade Type
              </Label>
              <Select defaultValue="auto" onValueChange={setUpgradeType}>
                <SelectTrigger className="col-span-3">
                  <SelectValue>{upgradeType}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {["auto", "manual"].map(version => (
                    <SelectItem key={version} value={version}>
                      {version}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {/* Upgrade To Version */}
            <div className="grid grid-cols-4 items-center gap-4">
              <Label
                htmlFor="upgradeTo"
                className="text-right text-sm font-medium"
              >
                Upgrade To
              </Label>
              <Select defaultValue={upgradeTo} onValueChange={setUpgradeTo}>
                <SelectTrigger className="col-span-3">
                  <SelectValue>
                    {upgradeTo ? `v${upgradeTo}` : "Select a version"}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {versionListGreaterThan?.length > 0 ? (
                    versionListGreaterThan.map(version => (
                      <SelectItem key={version} value={String(version)}>
                        v{version}
                      </SelectItem>
                    ))
                  ) : (
                    <div className="p-2 text-center text-sm text-muted-foreground">
                      No versions available above v
                      {agentDetails?.componentRevision}
                    </div>
                  )}
                </SelectContent>
              </Select>
            </div>
          </div>

          {/* Dialog Footer */}
          <AlertDialogFooter>
            <AlertDialogCancel className="border border-muted-foreground/20 hover:bg-muted/50">
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={handleUpgrade}
              className="bg-primary hover:bg-primary/90"
            >
              Upgrade
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Are you absolutely sure?</AlertDialogTitle>
            <AlertDialogDescription>
              This action cannot be undone. This will permanently delete the
              agent and remove all associated data.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={handleDelete}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

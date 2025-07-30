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
import { Worker } from "@/types/worker.ts";
import { CircleFadingArrowUp, Pause, Play, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

export default function WorkerManage() {
  const { appId, componentId = "", workerName = "" } = useParams();
  const navigate = useNavigate();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [showWorkerUpgrade, setShowWorkerUpgrade] = useState(false);
  const [upgradeTo, setUpgradeTo] = useState("0");
  const [upgradeType, setUpgradeType] = useState("Automatic");
  const [componentList, setComponentList] = useState<ComponentList>({});

  useEffect(() => {
    if (componentId && workerName) {
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
      API.workerService
        .getParticularWorker(appId!, componentId, workerName)
        .then(response => {
          const worker = response as Worker;
          setWorkerDetails(worker);
          setUpgradeTo(`${worker?.componentVersion}`);
        });
    }
  }, [componentId, workerName]);

  const handleUpgrade = () => {
    API.workerService
      .upgradeWorker(
        appId!,
        componentList.componentName!,
        workerDetails?.workerName,
        Number(upgradeTo),
        upgradeType,
      )
      .then(() => {
        toast({
          title: "Worker upgraded Initiated",
          duration: 3000,
        });
      });
  };

  const handleDelete = () => {
    API.workerService.deleteWorker(appId!, componentId, workerName).then(() => {
      toast({
        title: "Worker deleted successfully",
        duration: 3000,
        variant: "destructive",
      });
      navigate(`/app/${appId}/components/${componentId}`);
    });
  };

  const onResumeWorker = () => {
    API.workerService.resumeWorker(appId!, componentId, workerName).then(() => {
      toast({
        title: "Worker resumed",
        duration: 3000,
      });
    });
  };

  const onInterruptWorker = () => {
    API.workerService
      .interruptWorker(appId!, componentId, workerName)
      .then(() => {
        toast({
          title: "Worker interrupted",
          duration: 3000,
        });
      });
  };

  const versionListGreaterThan =
    componentList.versionList?.filter(
      version => version > workerDetails?.componentVersion,
    ) || [];

  return (
    <div className="flex flex-col items-center p-6 space-y-6 w-full max-w-4xl mx-auto">
      <Card className="w-full shadow-lg rounded-xl border border-border/30">
        <CardHeader>
          <CardTitle>Worker Execution</CardTitle>
          <CardDescription>
            Manage the worker and its execution.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Upgrade Worker</h3>
              <p className="text-sm text-muted-foreground">
                Upgrade Worker With New Component Version
              </p>
            </div>
            <Button
              variant="default"
              onClick={() => setShowWorkerUpgrade(true)}
              className="bg-blue-600 text-white hover:bg-blue-700"
            >
              <CircleFadingArrowUp className="mr-2 h-4 w-4" />
              Upgrade Worker
            </Button>
          </div>
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Interrupt Worker</h3>
              <p className="text-sm text-muted-foreground">
                Interrupts the execution of a running worker
              </p>
            </div>
            <Button variant="secondary" onClick={onInterruptWorker}>
              <Pause className="mr-2 h-4 w-4" />
              Interrupt Worker
            </Button>
          </div>
          <div className="flex items-center justify-between p-3 bg-muted/10 rounded-lg">
            <div>
              <h3 className="text-lg font-semibold">Resume Worker</h3>
              <p className="text-sm text-muted-foreground">
                Resumes the execution of an interrupted worker
              </p>
            </div>
            <Button variant="secondary" onClick={onResumeWorker}>
              <Play className="mr-2 h-4 w-4" />
              Resume Worker
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
              <h3 className="text-lg font-semibold">Delete this Worker</h3>
              <p className="text-sm text-muted-foreground">
                Once you delete a worker, there is no going back. Please be
                certain.
              </p>
            </div>
            <Button
              variant="destructive"
              onClick={() => setShowDeleteDialog(true)}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete Worker
            </Button>
          </div>
        </CardContent>
      </Card>
      <AlertDialog open={showWorkerUpgrade} onOpenChange={setShowWorkerUpgrade}>
        <AlertDialogContent className="sm:max-w-[600px]">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-xl font-semibold">
              Upgrade Worker
            </AlertDialogTitle>
            <AlertDialogDescription className="text-sm text-muted-foreground">
              This action cannot be undone. This will permanently upgrade the
              worker to the selected version.
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
                defaultValue={workerDetails.componentName || "N/A"}
                className="col-span-3 bg-muted/50"
                disabled
              />
            </div>

            {/* Current Component Version */}
            <div className="grid grid-cols-4 items-center gap-4">
              <Label
                htmlFor="componentVersion"
                className="text-right text-sm font-medium"
              >
                Current Version
              </Label>
              <Input
                defaultValue={workerDetails?.componentVersion}
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
                      {workerDetails?.componentVersion}
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
              worker and remove all associated data.
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

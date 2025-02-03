import {API} from "@/service";
import {useEffect, useState} from "react";
import {useNavigate, useParams} from "react-router-dom";
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
import {Button} from "@/components/ui/button";
import {Input} from "@/components/ui/input";
import {Card, CardContent, CardDescription, CardHeader, CardTitle,} from "@/components/ui/card";
import {CircleFadingArrowUp, Pause, Play, Trash2} from "lucide-react";
import {toast} from "@/hooks/use-toast";
import {Label} from "@/components/ui/label.tsx";
import {Worker} from "@/types/worker.ts";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue,} from "@/components/ui/select.tsx";
import {ComponentList} from "@/types/component.ts";

export default function WorkerManage() {
    const {componentId = "", workerName = ""} = useParams();
    const navigate = useNavigate();
    const [workerDetails, setWorkerDetails] = useState({} as Worker);
    const [showDeleteDialog, setShowDeleteDialog] = useState(false);
    const [showWorkerUpgrade, setShowWorkerUpgrade] = useState(false);
    const [upgradeTo, setUpgradeTo] = useState("");

    const [componentList, setComponentList] = useState<ComponentList>({});

    useEffect(() => {
        if (componentId && workerName) {
            API.getComponentByIdAsKey().then((response) => {
                setComponentList(response[componentId]);
            });
            API.getParticularWorker(componentId, workerName).then((response) => {
                setWorkerDetails(response);
                setUpgradeTo(`${workerDetails?.componentVersion}`);
            });
        }
    }, [componentId, workerName]);

    const handleUpgrade = () => {
        API.upgradeWorker(
            workerDetails?.workerId?.componentId,
            workerDetails?.workerId?.workerName,
            Number(upgradeTo)
        ).then(() => {
            toast({
                title: "Worker upgraded",
                duration: 3000,
            });
            // navigate(`/components/${componentId}/workers/${workerName}`);
        });
    };

    const handleDelete = () => {
        API.deleteWorker(componentId, workerName).then(() => {
            toast({
                title: "Worker deleted",
                duration: 3000,
            });
            navigate(`/components/${componentId}`);
        });
    };

    const onResumeWorker = () => {
        API.resumeWorker(componentId, workerName).then(() => {
            toast({
                title: "Worker resumed",
                duration: 3000,
            });
        });
    };

    const onInterruptWorker = () => {
        API.interruptWorker(componentId, workerName).then(() => {
            toast({
                title: "Worker interrupted",
                duration: 3000,
            });
        });
    };

    return (
        <div className="flex">
            <div className="flex-1 flex flex-col">

                <div className="p-10 space-y-6 w-10/12 mx-auto overflow-scroll h-[70vh]">
                    <div className="space-y-8 p-6">
                        <Card>
                            <CardHeader>
                                <CardTitle>Worker Execution</CardTitle>
                                <CardDescription>
                                    Manage the worker and its execution.
                                </CardDescription>
                            </CardHeader>
                            <CardContent className="space-y-6">
                                <div className="flex items-center justify-between">
                                    <div>
                                        <h3 className="text-lg font-semibold">Upgrade Worker</h3>
                                        <p className="text-sm text-muted-foreground">
                                            Upgrade Worker With New Component Version
                                        </p>
                                    </div>
                                    <Button
                                        variant="secondary"
                                        onClick={() => setShowWorkerUpgrade(true)}
                                    >
                                        <CircleFadingArrowUp className="mr-2 h-4 w-4"/>
                                        Upgrade Worker
                                    </Button>
                                </div>
                                <div className="flex items-center justify-between">
                                    <div>
                                        <h3 className="text-lg font-semibold">
                                            Interrupt Worker
                                        </h3>
                                        <p className="text-sm text-muted-foreground">
                                            Interrupts the execution of a running worker
                                        </p>
                                    </div>
                                    <Button variant="secondary" onClick={onInterruptWorker}>
                                        <Pause className="mr-2 h-4 w-4"/>
                                        Interrupt Worker
                                    </Button>
                                </div>
                                <div className="flex items-center justify-between">
                                    <div>
                                        <h3 className="text-lg font-semibold">Resume Worker</h3>
                                        <p className="text-sm text-muted-foreground">
                                            Resumes the execution of an interrupted worker
                                        </p>
                                    </div>
                                    <Button variant="secondary" onClick={onResumeWorker}>
                                        <Play className="mr-2 h-4 w-4"/>
                                        Resume Worker
                                    </Button>
                                </div>
                            </CardContent>
                        </Card>

                        <Card className="border-destructive/20 bg-destructive/5 dark:bg-destructive/10">
                            <CardHeader>
                                <CardTitle className="text-destructive">
                                    Danger Zone
                                </CardTitle>
                                <CardDescription>Proceed with caution.</CardDescription>
                            </CardHeader>
                            <CardContent>
                                <div className="flex items-center justify-between">
                                    <div className="mr-4">
                                        <h3 className="text-lg font-semibold">
                                            Delete this Worker
                                        </h3>
                                        <p className="text-sm text-muted-foreground">
                                            Once you delete a worker, there is no going back. Please
                                            be certain.
                                        </p>
                                    </div>
                                    <Button
                                        variant="destructive"
                                        onClick={() => setShowDeleteDialog(true)}
                                    >
                                        <Trash2 className="mr-2 h-4 w-4"/>
                                        Delete Worker
                                    </Button>
                                </div>
                            </CardContent>
                        </Card>

                        <AlertDialog
                            open={showWorkerUpgrade}
                            onOpenChange={setShowWorkerUpgrade}
                        >
                            <AlertDialogContent>
                                <AlertDialogHeader>
                                    <AlertDialogTitle>Upgrade Worker</AlertDialogTitle>
                                    <AlertDialogDescription>
                                        This action cannot be undone. This will permanently
                                        upgrade the worker
                                    </AlertDialogDescription>
                                </AlertDialogHeader>
                                <div className="grid gap-4 py-4">
                                    <div className="grid grid-cols-4 items-center gap-4">
                                        <Label htmlFor="componentId" className="text-right">
                                            Component ID
                                        </Label>
                                        <Input
                                            id="componentId"
                                            defaultValue={workerDetails?.workerId?.componentId}
                                            className="col-span-3"
                                            disabled={true}
                                        />
                                    </div>
                                    <div className="grid grid-cols-4 items-center gap-4">
                                        <Label htmlFor="componentId" className="text-right">
                                            Component Version
                                        </Label>
                                        <Input
                                            id="componentId"
                                            defaultValue={workerDetails?.componentVersion}
                                            className="col-span-3"
                                            disabled={true}
                                        />
                                    </div>
                                    <div className="grid grid-cols-4 items-center gap-4">
                                        <Label htmlFor="upgradeTo" className="text-right">
                                            Upgrade To
                                        </Label>
                                        <Select
                                            defaultValue={upgradeTo}
                                            onValueChange={setUpgradeTo}
                                        >
                                            <SelectTrigger id="upgradeTo" className="col-span-3">
                                                <SelectValue>v{upgradeTo}</SelectValue>
                                            </SelectTrigger>
                                            <SelectContent>
                                                {componentList.versionList
                                                    ?.filter(
                                                        (version) =>
                                                            version > workerDetails?.componentVersion
                                                    )
                                                    .map((version) => (
                                                        <SelectItem key={version} value={String(version)}>
                                                            v{version}
                                                        </SelectItem>
                                                    ))}
                                            </SelectContent>
                                        </Select>
                                    </div>
                                </div>
                                <AlertDialogFooter>
                                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                                    <AlertDialogAction onClick={handleUpgrade}>
                                        Upgrade
                                    </AlertDialogAction>
                                </AlertDialogFooter>
                            </AlertDialogContent>
                        </AlertDialog>

                        <AlertDialog
                            open={showDeleteDialog}
                            onOpenChange={setShowDeleteDialog}
                        >
                            <AlertDialogContent>
                                <AlertDialogHeader>
                                    <AlertDialogTitle>
                                        Are you absolutely sure?
                                    </AlertDialogTitle>
                                    <AlertDialogDescription>
                                        This action cannot be undone. This will permanently delete
                                        the worker and remove all associated data.
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
                </div>
            </div>
        </div>
    );
}

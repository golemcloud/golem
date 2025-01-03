import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";
import { API } from "@/service";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
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
import { Pause, Play, Trash2 } from "lucide-react";
import { toast } from "@/hooks/use-toast";

export default function WorkerManage() {
  const { componentId = "", workerName = "" } = useParams();
  const navigate = useNavigate();
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);

  const handleDelete = () => {
    API.deleteWorker(componentId, workerName).then(() => {
      toast({
        title: "Worker deleted",
      });
      navigate(`/components/${componentId}`);
    });
  };

  const onResumeWorker = () => {
    API.resumeWorker(componentId, workerName).then(() => {
      toast({
        title: "Worker resumed",
      });
    });
  };

  const onInterruptWorker = () => {
    API.interruptWorker(componentId, workerName).then(() => {
      toast({
        title: "Worker interrupted",
      });
    });
  };

  return (
    <ErrorBoundary>
      <div className="flex">
        <WorkerLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {workerName}
                </h1>
              </div>
            </div>
          </header>
          <div className="p-10 space-y-6 max-w-7xl mx-auto overflow-scroll h-[70vh]">
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
                      <h3 className="text-lg font-semibold">
                        Interrupt Worker
                      </h3>
                      <p className="text-sm text-muted-foreground">
                        Interrupts the execution of a running worker
                      </p>
                    </div>
                    <Button variant="secondary" onClick={onInterruptWorker}>
                      <Pause className="mr-2 h-4 w-4" />
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
                      <Play className="mr-2 h-4 w-4" />
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
                      <Trash2 className="mr-2 h-4 w-4" />
                      Delete Worker
                    </Button>
                  </div>
                </CardContent>
              </Card>

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
    </ErrorBoundary>
  );
}

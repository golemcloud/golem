import * as React from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { API } from "@/service";
import { Worker } from "@/types/worker";

export default function ComponentSettings() {
  const { toast } = useToast();
  const [showConfirmAllDialog, setShowConfirmAllDialog] = React.useState(false);
  const [isDeleting, setIsDeleting] = React.useState(false);
  const { componentId, appId } = useParams();
  const navigate = useNavigate();

  const handleDeleteAll = async () => {
    setIsDeleting(true);
    try {
      const response = await API.workerService.findWorker(
        appId!,
        componentId!,
        {
          count: 100,
          precise: true,
        },
      );

      await Promise.allSettled(
        response?.workers.map((worker: Worker) =>
          API.workerService.deleteWorker(
            appId!,
            componentId!,
            worker.workerName,
          ),
        ),
      );

      toast({
        title: "All workers deleted",
        description: `All workers for component ${componentId} have been deleted`,
        duration: 3000,
      });

      navigate(`/app/${appId}/components/${componentId}`);
    } catch (error) {
      console.error(error);
      toast({
        title: "Error",
        description: "Failed to delete some or all workers.",
        variant: "destructive",
      });
    } finally {
      setIsDeleting(false);
    }
  };

  return (
    <div className="flex flex-col items-center px-6 py-8">
      <Card className="max-w-4xl w-full border border-gray-200 shadow-md rounded-lg p-6">
        <CardTitle>
          <h1 className="text-2xl font-semibold mb-2">General Settings</h1>
        </CardTitle>
        <CardDescription className="text-gray-600">
          Manage your component settings.
        </CardDescription>
        <Separator className="my-4" />

        <CardContent className="py-6">
          {/* Danger Zone */}
          <Card className=" border border-red-500/30 bg-red-50 dark:bg-red-900/10 rounded-lg p-6 shadow-sm">
            <CardTitle>
              <h2 className="text-red-500">Danger Zone</h2>
            </CardTitle>
            <CardDescription>Proceed with caution.</CardDescription>
            <Separator className="my-4" />

            <CardContent className="px-0 py-2">
              <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4">
                <div>
                  <h3 className="text-lg font-medium">Delete All Workers</h3>
                  <p className="text-sm text-muted-foreground">
                    This will permanently delete all workers associated with
                    this component. This action cannot be undone.
                  </p>
                </div>

                <Dialog
                  open={showConfirmAllDialog}
                  onOpenChange={setShowConfirmAllDialog}
                >
                  <DialogTrigger asChild>
                    <Button variant="destructive">Delete All Workers</Button>
                  </DialogTrigger>
                  <DialogContent>
                    <DialogHeader className="text-center">
                      <DialogTitle className="text-xl font-semibold">
                        Are you sure?
                      </DialogTitle>
                      <DialogDescription className="text-gray-700">
                        This action cannot be undone. All associated workers
                        will be permanently removed.
                      </DialogDescription>
                    </DialogHeader>
                    <DialogFooter className="flex justify-center gap-4">
                      <Button
                        variant="outline"
                        onClick={() => setShowConfirmAllDialog(false)}
                        className="border-gray-300 text-gray-700 hover:bg-gray-100"
                      >
                        Cancel
                      </Button>
                      <Button
                        variant="destructive"
                        onClick={handleDeleteAll}
                        disabled={isDeleting}
                        className="px-5 py-2"
                      >
                        {isDeleting ? "Deleting..." : "Yes, delete all"}
                      </Button>
                    </DialogFooter>
                  </DialogContent>
                </Dialog>
              </div>
            </CardContent>
          </Card>
        </CardContent>
      </Card>
    </div>
  );
}

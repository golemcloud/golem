import { useEffect, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";
import { Trash2 } from "lucide-react";
import { HttpApiDefinition } from "@/types/golemManifest";

export default function APISettings() {
  const { toast } = useToast();
  const navigate = useNavigate();
  const { apiName, version, appId } = useParams();

  const [showConfirmDialog, setShowConfirmDialog] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [activeApiDetails, setActiveApiDetails] = useState<HttpApiDefinition>(
    {} as HttpApiDefinition,
  );

  useEffect(() => {
    if (apiName) {
      API.apiService.getApi(appId!, apiName).then(response => {
        const selectedApi = response.find(api => api.version === version);
        setActiveApiDetails(selectedApi!);
      });
    }
  }, [apiName, version]);

  const handleDelete = async () => {
    setIsDeleting(true);
    try {
      await API.apiService.deleteApi(
        appId!,
        activeApiDetails.id!,
        activeApiDetails.version,
      );
      toast({
        title: "Version deleted",
        description: `API version ${activeApiDetails.version} has been deleted successfully.`,
        duration: 3000,
      });
      navigate(`/app/${appId}/apis`);
      setShowConfirmDialog(false);
    } finally {
      setIsDeleting(false);
      setShowConfirmDialog(false);
    }
  };

  return (
    <ErrorBoundary>
      <div className="max-w-3xl mx-auto p-6 text-white space-y-6">
        <h1 className="text-3xl font-bold">API Settings</h1>
        <p className="text-gray-400">Manage your API settings</p>

        <Card className="border-red-500 bg-red-900/20">
          <CardHeader>
            <CardTitle className="text-red-500">Danger Zone</CardTitle>
            <CardDescription className="text-gray-400">
              Proceed with caution.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {[
              {
                title: `Delete API Version ${version}`,
                description:
                  "Once you delete an API, there is no going back. Please be certain.",
                action: () => setShowConfirmDialog(prev => !prev),
                confirm: showConfirmDialog,
                handler: () => handleDelete(),
              },
            ].map(({ title, description, action, confirm, handler }, index) => (
              <div
                key={index}
                className="flex items-center justify-between border-b border-red-500 pb-4 last:border-b-0"
              >
                <div>
                  <h3 className="text-lg font-semibold">{title}</h3>
                  <p className="text-sm text-muted-foreground pr-2">
                    {description}
                  </p>
                </div>
                <Button variant="destructive" onClick={action}>
                  <Trash2 className="mr-2 h-4 w-4" />
                  {title.split(" ")[0]}
                </Button>
                <Dialog open={confirm} onOpenChange={() => action()}>
                  <DialogContent>
                    <DialogHeader>
                      <DialogTitle>Are you sure?</DialogTitle>
                      <DialogDescription>{description}</DialogDescription>
                    </DialogHeader>
                    <DialogFooter>
                      <Button
                        variant="destructive"
                        onClick={handler}
                        disabled={isDeleting}
                      >
                        {isDeleting ? "Deleting..." : "Yes, delete"}
                      </Button>
                    </DialogFooter>
                  </DialogContent>
                </Dialog>
              </div>
            ))}
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
}

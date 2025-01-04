import { useEffect, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import ApiLeftNav from "./apiLeftNav.tsx";
import { API } from "@/service";
import { Api } from "@/types/api";
import ErrorBoundary from "@/components/errorBoundary";

export default function APISettings() {
  const { toast } = useToast();
  const navigate = useNavigate();
  const [showConfirmDialog, setShowConfirmDialog] = useState(false);
  const [showConfirmAllDialog, setShowConfirmAllDialog] = useState(false);
  const [showConfirmAllRoutes, setShowConfirmAllRoutes] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const { apiName } = useParams();
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  useEffect(() => {
    if (apiName) {
      API.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

  const handleDeleteVersion = async () => {
    setIsDeleting(true);
    API.deleteApi(activeApiDetails.id, activeApiDetails.version)
      .then(() => {
        toast({
          title: "Version deleted",
          description: `API version ${activeApiDetails.version} has been deleted successfully.`,
        });
        if (apiDetails.length === 1) {
          navigate(`/apis`);
        } else {
          setApiDetails(
            apiDetails.filter((api) => api.version !== activeApiDetails.version)
          );
          navigate(`/apis/${apiName}`);
        }
        setShowConfirmDialog(false);
        setIsDeleting(false);
      })
      .catch(() => {
        setIsDeleting(false);
      });
  };

  const handleDeleteAll = async () => {
    setIsDeleting(true);
    const promises = apiDetails.map((api) =>
      API.deleteApi(api.id, api.version)
    );
    Promise.all(promises)
      .then(() => {
        toast({
          title: "All versions deleted",
          description: "All API versions have been deleted successfully.",
        });
        setShowConfirmAllDialog(false);
        navigate(`/apis`);
        setIsDeleting(false);
      })
      .catch(() => {
        setIsDeleting(false);
      });
  };

  const handleDeleteAllRoutes = async () => {
    setIsDeleting(true);
    const payload = {
      ...activeApiDetails,
      routes: [],
    };
    API.putApi(activeApiDetails.id, activeApiDetails.version, payload)
      .then(() => {
        toast({
          title: "All routes deleted",
          description: "All routes have been deleted successfully.",
        });
        navigate(`/apis/${apiName}`);
        setShowConfirmAllRoutes(false);
        setIsDeleting(false);
      })
      .catch(() => {
        setIsDeleting(false);
      });
  };

  return (
    <ErrorBoundary>
      <div className="flex bg-background text-foreground">
        <ApiLeftNav />
        <div className="flex-1">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {apiName}
                </h1>
                <div className="flex items-center gap-2">
                  {activeApiDetails.version && (
                    <Select
                      defaultValue={activeApiDetails.version}
                      onValueChange={(version) => {
                        const selectedApi = apiDetails.find(
                          (api) => api.version === version
                        );
                        if (selectedApi) {
                          setActiveApiDetails(selectedApi);
                        }
                      }}
                    >
                      <SelectTrigger className="w-28">
                        <SelectValue>{activeApiDetails.version}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {apiDetails.map((api) => (
                          <SelectItem value={api.version} key={api.version}>
                            {api.version}{" "}
                            {api.draft ? "(Draft)" : "(Published)"}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                </div>
              </div>
            </div>
          </header>

          <div className="overflow-y-auto h-[85vh] max-w-4xl mx-auto p-8">
            <h1 className="text-3xl font-semibold mb-2">API Settings</h1>
            <p className="text-muted-foreground text-lg mb-8">
              Manage your API settings
            </p>

            <div className="border border-destructive/20 rounded-lg bg-destructive/10 p-6">
              <h2 className="text-2xl font-semibold mb-4 text-destructive">
                Danger Zone
              </h2>
              <p className="text-muted-foreground mb-8">
                Proceed with caution.
              </p>

              <div className="space-y-8">
                <div className="flex items-center justify-between">
                  <div>
                    <h3 className="text-xl font-semibold mb-2">
                      Delete API Version {activeApiDetails.version}
                    </h3>
                    <p className="text-muted-foreground">
                      Once you delete an API, there is no going back. Please be
                      certain.
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    className="border-destructive/20 text-destructive hover:bg-destructive/10"
                    onClick={() => setShowConfirmDialog(true)}
                  >
                    Delete Version {activeApiDetails.version}
                  </Button>
                </div>

                <div className="flex items-center justify-between">
                  <div>
                    <h3 className="text-xl font-semibold mb-2">
                      Delete all API Versions
                    </h3>
                    <p className="text-muted-foreground">
                      Once you delete all API versions, there is no going back.
                      Please be certain.
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    className="border-destructive/20 text-destructive hover:bg-destructive/10"
                    onClick={() => setShowConfirmAllDialog(true)}
                  >
                    Delete All Versions
                  </Button>
                </div>
                <div className="flex items-center justify-between">
                  <div>
                    <h3 className="text-xl font-semibold mb-2">
                      Delete All Routes
                    </h3>
                    <p className="text-muted-foreground">
                      Once you delete all routes, there is no going back. Please
                      be certain.
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    className="border-destructive/20 text-destructive hover:bg-destructive/10"
                    onClick={() => setShowConfirmAllRoutes(true)}
                  >
                    Delete All Routes
                  </Button>
                </div>
              </div>
            </div>

            <Dialog
              open={showConfirmDialog}
              onOpenChange={setShowConfirmDialog}
            >
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>Are you sure you want to delete?</DialogTitle>
                  <DialogDescription>
                    This action cannot be undone. This will permanently delete
                    API version {activeApiDetails.version}.
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <Button
                    variant="destructive"
                    onClick={handleDeleteVersion}
                    disabled={isDeleting}
                  >
                    {isDeleting ? "Deleting..." : "Yes, delete"}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>

            <Dialog
              open={showConfirmAllDialog}
              onOpenChange={setShowConfirmAllDialog}
            >
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>
                    Are you sure you want to delete all versions?
                  </DialogTitle>
                  <DialogDescription>
                    This action cannot be undone. This will permanently delete
                    all API versions and remove all associated data.
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <Button
                    variant="destructive"
                    onClick={handleDeleteAll}
                    disabled={isDeleting}
                  >
                    {isDeleting ? "Deleting..." : "Yes, delete all"}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>

            <Dialog
              open={showConfirmAllRoutes}
              onOpenChange={setShowConfirmAllRoutes}
            >
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>
                    Are you sure you want to delete all routes?
                  </DialogTitle>
                  <DialogDescription>
                    This action cannot be undone. This will permanently delete
                    all routes and remove all associated data.
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <Button
                    variant="destructive"
                    onClick={handleDeleteAllRoutes}
                    disabled={isDeleting}
                  >
                    {isDeleting ? "Deleting..." : "Yes, delete all"}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}

import { useEffect, useState } from "react";
import {
  Cloud,
  Globe,
  Monitor,
  Plus,
  Search,
  Server,
  Star,
  Trash,
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { API } from "@/service";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge";
import { useToast } from "@/hooks/use-toast";
import { ManifestEnvironment } from "@/types/environment";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export default function Environments() {
  const navigate = useNavigate();
  const { toast } = useToast();
  const [environments, setEnvironments] = useState<
    Record<string, ManifestEnvironment>
  >({});
  const [searchTerm, setSearchTerm] = useState("");
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [envToDelete, setEnvToDelete] = useState<string | null>(null);
  const { appId } = useParams<{ appId: string }>();

  const fetchEnvironments = async () => {
    try {
      const envs = await API.environmentService.getEnvironments(appId!);
      setEnvironments(envs);
    } catch (error) {
      console.error("Failed to fetch environments:", error);
      toast({
        title: "Error",
        description: "Failed to load environments",
        variant: "destructive",
      });
    }
  };

  useEffect(() => {
    fetchEnvironments();
  }, [appId]);

  const filteredEnvironments = Object.entries(environments).filter(([name]) =>
    name.toLowerCase().includes(searchTerm.toLowerCase()),
  );

  const handleDelete = async () => {
    if (!envToDelete) return;

    try {
      await API.environmentService.deleteEnvironment(appId!, envToDelete);
      toast({
        title: "Environment Deleted",
        description: `Environment "${envToDelete}" has been deleted`,
        duration: 3000,
      });
      setDeleteDialogOpen(false);
      setEnvToDelete(null);
      fetchEnvironments();
    } catch (error) {
      console.error("Failed to delete environment:", error);
      toast({
        title: "Delete Failed",
        description:
          error instanceof Error
            ? error.message
            : "Failed to delete environment",
        variant: "destructive",
        duration: 3000,
      });
    }
  };

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-6 py-10">
        <div className="flex items-center justify-between gap-4 mb-8">
          <div>
            <h1 className="text-2xl font-bold tracking-tight">Environments</h1>
            <p className="text-sm text-muted-foreground mt-1">
              Manage deployment targets and configurations
            </p>
          </div>
          <Button
            onClick={() => navigate(`/app/${appId}/environments/create`)}
            variant="default"
          >
            <Plus className="h-5 w-5 mr-2" />
            <span>New Environment</span>
          </Button>
        </div>

        <div className="relative mb-6">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-5 w-5" />
          <Input
            type="text"
            placeholder="Search environments..."
            value={searchTerm}
            onChange={e => setSearchTerm(e.target.value)}
            className="pl-10"
          />
        </div>

        {filteredEnvironments.length > 0 ? (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[75vh]">
            {filteredEnvironments.map(([name, env]) => (
              <EnvironmentCard
                key={name}
                name={name}
                environment={env}
                onDelete={() => {
                  setEnvToDelete(name);
                  setDeleteDialogOpen(true);
                }}
              />
            ))}
          </div>
        ) : (
          <Card className="border-2 border-dashed">
            <div className="flex flex-col items-center justify-center py-16">
              <div className="p-4 rounded-full bg-muted mb-4">
                <Globe className="h-8 w-8 text-muted-foreground" />
              </div>
              <h3 className="text-lg font-semibold mb-2">
                {searchTerm ? "No Environments Found" : "No Environments Yet"}
              </h3>
              <p className="text-sm text-muted-foreground mb-6 text-center max-w-sm">
                {searchTerm
                  ? "Try adjusting your search terms"
                  : "Create your first environment to get started"}
              </p>
              {!searchTerm && (
                <Button
                  onClick={() => navigate(`/app/${appId}/environments/create`)}
                >
                  <Plus className="h-4 w-4 mr-2" />
                  Create Environment
                </Button>
              )}
            </div>
          </Card>
        )}
      </div>

      <Dialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete Environment</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete the environment{" "}
              <strong className="text-foreground">
                &quot;{envToDelete}&quot;
              </strong>
              ?
              <br />
              This action cannot be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setDeleteDialogOpen(false)}
            >
              Cancel
            </Button>
            <Button variant="destructive" onClick={handleDelete}>
              Delete Environment
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ErrorBoundary>
  );
}

interface EnvironmentCardProps {
  name: string;
  environment: ManifestEnvironment;
  onDelete: () => void;
}

const EnvironmentCard = ({
  name,
  environment,
  onDelete,
}: EnvironmentCardProps) => {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();

  const getServerIcon = () => {
    if (!environment.server) {
      return <Monitor className="h-5 w-5 text-primary" />;
    }
    if (environment.server.type === "builtin") {
      return environment.server.value === "cloud" ? (
        <Cloud className="h-5 w-5 text-primary" />
      ) : (
        <Monitor className="h-5 w-5 text-primary" />
      );
    }
    return <Server className="h-5 w-5 text-primary" />;
  };

  const getServerLabel = () => {
    if (!environment.server) {
      return "Local";
    }
    if (environment.server.type === "builtin") {
      return environment.server.value === "cloud" ? "Cloud" : "Local";
    }
    return "Custom";
  };

  const getPresetsCount = () => {
    if (!environment.componentPresets) return 0;
    return typeof environment.componentPresets === "string"
      ? 1
      : environment.componentPresets.length;
  };

  return (
    <Card className="from-background to-muted bg-gradient-to-br border-border hover:shadow-lg transition-all relative">
      <div
        className="cursor-pointer"
        onClick={() => navigate(`/app/${appId}/environments/${name}`)}
      >
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-base font-semibold flex items-center gap-2">
            {getServerIcon()}
            {name}
          </CardTitle>
          {environment.default && (
            <Badge
              variant="secondary"
              className="bg-emerald-500 text-white border-emerald-400"
            >
              <Star className="h-3 w-3 mr-1 fill-current" />
              Default
            </Badge>
          )}
        </CardHeader>
        <CardContent>
          <div className="space-y-3">
            <div className="flex items-center justify-between text-sm">
              <span className="text-muted-foreground">Server</span>
              <Badge variant="outline">{getServerLabel()}</Badge>
            </div>

            {environment.account && (
              <div className="flex items-center justify-between text-sm">
                <span className="text-muted-foreground">Account</span>
                <span className="text-xs font-mono">{environment.account}</span>
              </div>
            )}

            <div className="flex items-center justify-between text-sm">
              <span className="text-muted-foreground">Presets</span>
              <Badge variant="secondary">{getPresetsCount()}</Badge>
            </div>

            {environment.deployment && (
              <div className="flex flex-wrap gap-1 mt-2">
                {environment.deployment.compatibilityCheck && (
                  <Badge variant="outline" className="text-xs">
                    Compatibility Check
                  </Badge>
                )}
                {environment.deployment.versionCheck && (
                  <Badge variant="outline" className="text-xs">
                    Version Check
                  </Badge>
                )}
                {environment.deployment.securityOverrides && (
                  <Badge variant="outline" className="text-xs">
                    Security Overrides
                  </Badge>
                )}
              </div>
            )}
          </div>
        </CardContent>
      </div>

      {!environment.default && (
        <div className="absolute top-2 right-2">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 text-destructive hover:text-destructive hover:bg-destructive/10"
            onClick={e => {
              e.stopPropagation();
              onDelete();
            }}
          >
            <Trash className="h-4 w-4" />
          </Button>
        </div>
      )}
    </Card>
  );
};

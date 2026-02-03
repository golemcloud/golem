import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowRight, Cloud, Globe, Monitor, Server, Star } from "lucide-react";
import { useEffect, useState } from "react";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge";
import { ManifestEnvironment } from "@/types/environment";

export function EnvironmentSection() {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  const [defaultEnvironment, setDefaultEnvironment] = useState<{
    name: string;
    environment: ManifestEnvironment;
  } | null>(null);
  const [environmentCount, setEnvironmentCount] = useState(0);

  useEffect(() => {
    const fetchEnvironments = async () => {
      try {
        const environments = await API.environmentService.getEnvironments(
          appId!,
        );
        setEnvironmentCount(Object.keys(environments).length);

        const defaultEnv = await API.environmentService.getDefaultEnvironment(
          appId!,
        );
        setDefaultEnvironment(defaultEnv || null);
      } catch (error) {
        console.error("Error fetching environments:", error);
      }
    };

    fetchEnvironments();
  }, [appId]);

  const getServerIcon = () => {
    if (!defaultEnvironment?.environment.server) {
      return <Monitor className="h-5 w-5 text-muted-foreground" />;
    }
    if (defaultEnvironment.environment.server.type === "builtin") {
      return defaultEnvironment.environment.server.value === "cloud" ? (
        <Cloud className="h-5 w-5 text-muted-foreground" />
      ) : (
        <Monitor className="h-5 w-5 text-muted-foreground" />
      );
    }
    return <Server className="h-5 w-5 text-muted-foreground" />;
  };

  const getServerLabel = () => {
    if (!defaultEnvironment?.environment.server) {
      return "Local";
    }
    if (defaultEnvironment.environment.server.type === "builtin") {
      return defaultEnvironment.environment.server.value === "cloud"
        ? "Cloud"
        : "Local";
    }
    return "Custom";
  };

  const getPresetsCount = () => {
    if (!defaultEnvironment?.environment.componentPresets) return 0;
    return typeof defaultEnvironment.environment.componentPresets === "string"
      ? 1
      : defaultEnvironment.environment.componentPresets.length;
  };

  return (
    <ErrorBoundary>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between">
          <CardTitle className="text-xl font-semibold flex items-center gap-2 text-primary">
            <Globe className="w-5 h-5 text-muted-foreground" />
            Environments
          </CardTitle>
          <Button
            variant="ghost"
            className="text-sm font-medium"
            size="sm"
            onClick={() => navigate(`/app/${appId}/environments`)}
          >
            View All
            <ArrowRight className="w-4 h-4 ml-1" />
          </Button>
        </CardHeader>
        <CardContent className="space-y-2">
          {environmentCount > 0 ? (
            <>
              <div className="flex items-center justify-between text-sm mb-2">
                <span className="text-muted-foreground">
                  Total Environments
                </span>
                <Badge variant="secondary">{environmentCount}</Badge>
              </div>

              {defaultEnvironment && (
                <div
                  className="border rounded-lg p-3 hover:bg-muted/50 cursor-pointer bg-gradient-to-br from-background to-muted hover:shadow-lg transition-all"
                  onClick={() => {
                    navigate(
                      `/app/${appId}/environments/${defaultEnvironment.name}`,
                    );
                  }}
                >
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-2">
                      {getServerIcon()}
                      <span className="font-medium">
                        {defaultEnvironment.name}
                      </span>
                    </div>
                    <Badge
                      variant="secondary"
                      className="bg-emerald-500 text-white border-emerald-400"
                    >
                      <Star className="h-3 w-3 mr-1 fill-current" />
                      Default
                    </Badge>
                  </div>
                  <div className="flex items-center gap-4 text-xs text-muted-foreground">
                    <span>Server: {getServerLabel()}</span>
                    <span>•</span>
                    <span>Presets: {getPresetsCount()}</span>
                  </div>
                </div>
              )}

              {!defaultEnvironment && (
                <div className="border border-dashed rounded-lg p-4 text-center">
                  <p className="text-sm text-muted-foreground mb-2">
                    No default environment set
                  </p>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => navigate(`/app/${appId}/environments`)}
                  >
                    Set Default
                  </Button>
                </div>
              )}
            </>
          ) : (
            <div className="border-2 border-dashed border-gray-200 rounded-lg p-8 flex flex-col items-center justify-center">
              <div className="h-12 w-12 bg-gray-100 rounded-lg flex items-center justify-center mb-3">
                <Globe className="h-6 w-6 text-gray-400" />
              </div>
              <h3 className="text-base font-semibold mb-1 text-center">
                No Environments
              </h3>
              <p className="text-gray-500 text-sm mb-4 text-center">
                Create your first environment
              </p>
              <Button
                size="sm"
                onClick={() => navigate(`/app/${appId}/environments/create`)}
              >
                Create Environment
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}

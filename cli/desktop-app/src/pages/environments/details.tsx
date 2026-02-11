import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Loader2, Star } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import ErrorBoundary from "@/components/errorBoundary";
import { API } from "@/service";
import { useToast } from "@/hooks/use-toast";
import {
  EnvironmentFormData,
  formDataToManifestEnvironment,
  manifestEnvironmentToFormData,
} from "@/types/environment";

export default function EnvironmentDetails() {
  const navigate = useNavigate();
  const { toast } = useToast();
  const { appId, envName } = useParams<{ appId: string; envName: string }>();
  const [loading, setLoading] = useState(false);
  const [fetching, setFetching] = useState(true);
  const [formData, setFormData] = useState<EnvironmentFormData | null>(null);

  useEffect(() => {
    const fetchEnvironment = async () => {
      try {
        const env = await API.environmentService.getEnvironment(
          appId!,
          envName!,
        );
        if (!env) {
          toast({
            title: "Environment Not Found",
            description: `Environment "${envName}" does not exist`,
            variant: "destructive",
          });
          navigate(`/app/${appId}/environments`);
          return;
        }
        const data = manifestEnvironmentToFormData(envName!, env);
        setFormData(data);
      } catch (error) {
        console.error("Failed to fetch environment:", error);
        toast({
          title: "Error",
          description: "Failed to load environment",
          variant: "destructive",
        });
      } finally {
        setFetching(false);
      }
    };

    fetchEnvironment();
  }, [appId, envName]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!formData) return;

    setLoading(true);

    try {
      const manifestEnv = formDataToManifestEnvironment(formData);
      await API.environmentService.updateEnvironment(
        appId!,
        envName!,
        manifestEnv,
      );

      toast({
        title: "Environment Updated",
        description: `Environment "${envName}" has been updated successfully`,
        duration: 3000,
      });

      navigate(`/app/${appId}/environments`);
    } catch (error) {
      console.error("Failed to update environment:", error);
      toast({
        title: "Update Failed",
        description:
          error instanceof Error
            ? error.message
            : "Failed to update environment",
        variant: "destructive",
        duration: 3000,
      });
      setLoading(false);
    }
  };

  const handleSetDefault = async () => {
    try {
      await API.environmentService.setDefaultEnvironment(appId!, envName!);
      toast({
        title: "Default Updated",
        description: `"${envName}" is now the default environment`,
        duration: 3000,
      });
      // Refresh the form data
      const env = await API.environmentService.getEnvironment(appId!, envName!);
      if (env) {
        const data = manifestEnvironmentToFormData(envName!, env);
        setFormData(data);
      }
    } catch (error) {
      console.error("Failed to set default:", error);
      toast({
        title: "Failed",
        description: "Failed to set as default environment",
        variant: "destructive",
      });
    }
  };

  if (fetching || !formData) {
    return (
      <div className="flex items-center justify-center h-96">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-6 py-10 max-w-4xl">
        <div className="mb-8">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => navigate(`/app/${appId}/environments`)}
            className="mb-4"
          >
            <ArrowLeft className="h-4 w-4 mr-2" />
            Back to Environments
          </Button>
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-2xl font-bold tracking-tight flex items-center gap-2">
                {envName}
                {formData.isDefault && (
                  <Star className="h-5 w-5 text-emerald-500 fill-current" />
                )}
              </h1>
              <p className="text-sm text-muted-foreground mt-1">
                Edit environment configuration
              </p>
            </div>
            {!formData.isDefault && (
              <Button variant="outline" size="sm" onClick={handleSetDefault}>
                <Star className="h-4 w-4 mr-2" />
                Set as Default
              </Button>
            )}
          </div>
        </div>

        <form onSubmit={handleSubmit} className="space-y-6">
          {/* General Settings */}
          <Card>
            <CardHeader>
              <CardTitle>General</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label>Environment Name</Label>
                <Input value={envName} disabled />
                <p className="text-xs text-muted-foreground">
                  Environment name cannot be changed
                </p>
              </div>

              <div className="flex items-center space-x-2">
                <Checkbox
                  id="isDefault"
                  checked={formData.isDefault}
                  disabled
                />
                <Label htmlFor="isDefault" className="text-muted-foreground">
                  Default environment
                </Label>
              </div>

              <div className="space-y-2">
                <Label htmlFor="account">Account (Optional)</Label>
                <Input
                  id="account"
                  value={formData.account || ""}
                  onChange={e =>
                    setFormData({ ...formData, account: e.target.value })
                  }
                  placeholder="Account identifier"
                />
              </div>
            </CardContent>
          </Card>

          {/* Server Configuration */}
          <Card>
            <CardHeader>
              <CardTitle>Server Configuration</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <RadioGroup
                value={formData.serverType}
                onValueChange={(value: "local" | "cloud" | "custom") =>
                  setFormData({ ...formData, serverType: value })
                }
              >
                <div className="flex items-center space-x-2">
                  <RadioGroupItem value="local" id="local" />
                  <Label htmlFor="local" className="cursor-pointer">
                    Local Server
                  </Label>
                </div>
                <div className="flex items-center space-x-2">
                  <RadioGroupItem value="cloud" id="cloud" />
                  <Label htmlFor="cloud" className="cursor-pointer">
                    Golem Cloud
                  </Label>
                </div>
                <div className="flex items-center space-x-2">
                  <RadioGroupItem value="custom" id="custom" />
                  <Label htmlFor="custom" className="cursor-pointer">
                    Custom Server
                  </Label>
                </div>
              </RadioGroup>

              {formData.serverType === "custom" && (
                <div className="space-y-4 pl-6 border-l-2 border-muted">
                  <div className="space-y-2">
                    <Label htmlFor="customUrl">Server URL *</Label>
                    <Input
                      id="customUrl"
                      value={formData.customServerUrl || ""}
                      onChange={e =>
                        setFormData({
                          ...formData,
                          customServerUrl: e.target.value,
                        })
                      }
                      placeholder="https://my-server.example.com"
                      required={formData.serverType === "custom"}
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="customWorkerUrl">
                      Worker URL (Optional)
                    </Label>
                    <Input
                      id="customWorkerUrl"
                      value={formData.customServerWorkerUrl || ""}
                      onChange={e =>
                        setFormData({
                          ...formData,
                          customServerWorkerUrl: e.target.value,
                        })
                      }
                      placeholder="https://worker.example.com"
                    />
                  </div>

                  <div className="flex items-center space-x-2">
                    <Checkbox
                      id="allowInsecure"
                      checked={formData.customServerAllowInsecure || false}
                      onCheckedChange={checked =>
                        setFormData({
                          ...formData,
                          customServerAllowInsecure: checked as boolean,
                        })
                      }
                    />
                    <Label htmlFor="allowInsecure" className="cursor-pointer">
                      Allow insecure connections
                    </Label>
                  </div>

                  <Separator />

                  <div className="space-y-2">
                    <Label>Authentication</Label>
                    <RadioGroup
                      value={formData.customServerAuthType || "oauth2"}
                      onValueChange={(value: "oauth2" | "static") =>
                        setFormData({
                          ...formData,
                          customServerAuthType: value,
                        })
                      }
                    >
                      <div className="flex items-center space-x-2">
                        <RadioGroupItem value="oauth2" id="oauth2" />
                        <Label htmlFor="oauth2" className="cursor-pointer">
                          OAuth2
                        </Label>
                      </div>
                      <div className="flex items-center space-x-2">
                        <RadioGroupItem value="static" id="static" />
                        <Label htmlFor="static" className="cursor-pointer">
                          Static Token
                        </Label>
                      </div>
                    </RadioGroup>
                  </div>

                  {formData.customServerAuthType === "static" && (
                    <div className="space-y-2">
                      <Label htmlFor="staticToken">Static Token *</Label>
                      <Input
                        id="staticToken"
                        type="password"
                        value={formData.customServerStaticToken || ""}
                        onChange={e =>
                          setFormData({
                            ...formData,
                            customServerStaticToken: e.target.value,
                          })
                        }
                        placeholder="Enter static authentication token"
                        required={
                          formData.serverType === "custom" &&
                          formData.customServerAuthType === "static"
                        }
                      />
                    </div>
                  )}
                </div>
              )}
            </CardContent>
          </Card>

          {/* Component Presets */}
          <Card>
            <CardHeader>
              <CardTitle>Component Presets</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="space-y-2">
                <Label htmlFor="presets">Presets (comma-separated)</Label>
                <Input
                  id="presets"
                  value={formData.componentPresets.join(", ")}
                  onChange={e =>
                    setFormData({
                      ...formData,
                      componentPresets: e.target.value
                        .split(",")
                        .map(s => s.trim())
                        .filter(Boolean),
                    })
                  }
                  placeholder="e.g., preset1, preset2"
                />
              </div>
            </CardContent>
          </Card>

          {/* CLI Options */}
          <Card>
            <CardHeader>
              <CardTitle>CLI Options</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="format">Output Format</Label>
                <Select
                  value={formData.cliFormat || "default"}
                  onValueChange={value =>
                    setFormData({
                      ...formData,
                      cliFormat:
                        value === "default"
                          ? undefined
                          : (value as typeof formData.cliFormat),
                    })
                  }
                >
                  <SelectTrigger id="format">
                    <SelectValue placeholder="Default" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="default">Default</SelectItem>
                    <SelectItem value="text">Text</SelectItem>
                    <SelectItem value="json">JSON</SelectItem>
                    <SelectItem value="yaml">YAML</SelectItem>
                    <SelectItem value="pretty-json">Pretty JSON</SelectItem>
                    <SelectItem value="pretty-yaml">Pretty YAML</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div className="space-y-3">
                <div className="flex items-center space-x-2">
                  <Checkbox
                    id="autoConfirm"
                    checked={formData.cliAutoConfirm || false}
                    onCheckedChange={checked =>
                      setFormData({
                        ...formData,
                        cliAutoConfirm: checked as boolean,
                      })
                    }
                  />
                  <Label htmlFor="autoConfirm" className="cursor-pointer">
                    Auto-confirm operations
                  </Label>
                </div>

                <div className="flex items-center space-x-2">
                  <Checkbox
                    id="redeployAgents"
                    checked={formData.cliRedeployAgents || false}
                    onCheckedChange={checked =>
                      setFormData({
                        ...formData,
                        cliRedeployAgents: checked as boolean,
                      })
                    }
                  />
                  <Label htmlFor="redeployAgents" className="cursor-pointer">
                    Redeploy agents on changes
                  </Label>
                </div>

                <div className="flex items-center space-x-2">
                  <Checkbox
                    id="reset"
                    checked={formData.cliReset || false}
                    onCheckedChange={checked =>
                      setFormData({
                        ...formData,
                        cliReset: checked as boolean,
                      })
                    }
                  />
                  <Label htmlFor="reset" className="cursor-pointer">
                    Reset on deploy
                  </Label>
                </div>
              </div>
            </CardContent>
          </Card>

          {/* Deployment Options */}
          <Card>
            <CardHeader>
              <CardTitle>Deployment Options</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center space-x-2">
                <Checkbox
                  id="compatibilityCheck"
                  checked={formData.deploymentCompatibilityCheck || false}
                  onCheckedChange={checked =>
                    setFormData({
                      ...formData,
                      deploymentCompatibilityCheck: checked as boolean,
                    })
                  }
                />
                <Label htmlFor="compatibilityCheck" className="cursor-pointer">
                  Compatibility check
                </Label>
              </div>

              <div className="flex items-center space-x-2">
                <Checkbox
                  id="versionCheck"
                  checked={formData.deploymentVersionCheck || false}
                  onCheckedChange={checked =>
                    setFormData({
                      ...formData,
                      deploymentVersionCheck: checked as boolean,
                    })
                  }
                />
                <Label htmlFor="versionCheck" className="cursor-pointer">
                  Version check
                </Label>
              </div>

              <div className="flex items-center space-x-2">
                <Checkbox
                  id="securityOverrides"
                  checked={formData.deploymentSecurityOverrides || false}
                  onCheckedChange={checked =>
                    setFormData({
                      ...formData,
                      deploymentSecurityOverrides: checked as boolean,
                    })
                  }
                />
                <Label htmlFor="securityOverrides" className="cursor-pointer">
                  Allow security overrides
                </Label>
              </div>
            </CardContent>
          </Card>

          {/* Actions */}
          <div className="flex justify-end gap-3">
            <Button
              type="button"
              variant="outline"
              onClick={() => navigate(`/app/${appId}/environments`)}
              disabled={loading}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={loading}>
              {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              Save Changes
            </Button>
          </div>
        </form>
      </div>
    </ErrorBoundary>
  );
}

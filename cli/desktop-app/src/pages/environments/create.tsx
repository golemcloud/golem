import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Loader2 } from "lucide-react";
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
  ManifestEnvironment,
} from "@/types/environment";

export default function CreateEnvironment() {
  const navigate = useNavigate();
  const { toast } = useToast();
  const { appId } = useParams<{ appId: string }>();
  const [loading, setLoading] = useState(false);
  const [copyFromEnv, setCopyFromEnv] = useState<string>("");
  const [existingEnvironments, setExistingEnvironments] = useState<
    Record<string, ManifestEnvironment>
  >({});

  const [formData, setFormData] = useState<EnvironmentFormData>({
    name: "",
    isDefault: false,
    serverType: "local",
    componentPresets: [],
    customServerAuthType: "oauth2",
  });

  useEffect(() => {
    const fetchEnvironments = async () => {
      try {
        const envs = await API.environmentService.getEnvironments(appId!);
        setExistingEnvironments(envs);
      } catch (error) {
        console.error("Failed to fetch environments:", error);
      }
    };
    fetchEnvironments();
  }, [appId]);

  useEffect(() => {
    if (copyFromEnv && existingEnvironments[copyFromEnv]) {
      const env = existingEnvironments[copyFromEnv];
      const copiedData: EnvironmentFormData = {
        name: "",
        isDefault: false,
        account: env.account,
        serverType: "local",
        componentPresets:
          typeof env.componentPresets === "string"
            ? [env.componentPresets]
            : env.componentPresets || [],
        cliFormat: env.cli?.format,
        cliAutoConfirm: env.cli?.autoConfirm,
        cliRedeployAgents: env.cli?.redeployAgents,
        cliReset: env.cli?.reset,
        deploymentCompatibilityCheck: env.deployment?.compatibilityCheck,
        deploymentVersionCheck: env.deployment?.versionCheck,
        deploymentSecurityOverrides: env.deployment?.securityOverrides,
      };

      if (env.server) {
        if (env.server.type === "builtin") {
          copiedData.serverType = env.server.value;
        } else {
          copiedData.serverType = "custom";
          copiedData.customServerUrl = env.server.value.url;
          copiedData.customServerWorkerUrl = env.server.value.workerUrl;
          copiedData.customServerAllowInsecure = env.server.value.allowInsecure;
          if ("oauth2" in env.server.value.auth) {
            copiedData.customServerAuthType = "oauth2";
          } else {
            copiedData.customServerAuthType = "static";
            copiedData.customServerStaticToken =
              env.server.value.auth.staticToken;
          }
        }
      }

      setFormData(copiedData);
    }
  }, [copyFromEnv, existingEnvironments]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);

    try {
      // Validate environment name
      if (!formData.name || !/^[a-z][a-z0-9-]*$/.test(formData.name)) {
        toast({
          title: "Invalid Name",
          description:
            "Environment name must be lowercase-kebab-case (e.g., 'my-environment')",
          variant: "destructive",
        });
        setLoading(false);
        return;
      }

      const manifestEnv = formDataToManifestEnvironment(formData);
      await API.environmentService.createEnvironment(
        appId!,
        formData.name,
        manifestEnv,
      );

      toast({
        title: "Environment Created",
        description: `Environment "${formData.name}" has been created successfully`,
        duration: 3000,
      });

      navigate(`/app/${appId}/environments`);
    } catch (error) {
      console.error("Failed to create environment:", error);
      toast({
        title: "Creation Failed",
        description:
          error instanceof Error
            ? error.message
            : "Failed to create environment",
        variant: "destructive",
        duration: 3000,
      });
      setLoading(false);
    }
  };

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
          <h1 className="text-2xl font-bold tracking-tight">
            Create New Environment
          </h1>
          <p className="text-sm text-muted-foreground mt-1">
            Configure a new deployment target for your application
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-6">
          {/* Copy from Existing */}
          {Object.keys(existingEnvironments).length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">
                  Copy from Existing Environment
                </CardTitle>
              </CardHeader>
              <CardContent>
                <Select
                  value={copyFromEnv || "none"}
                  onValueChange={value =>
                    setCopyFromEnv(value === "none" ? "" : value)
                  }
                >
                  <SelectTrigger>
                    <SelectValue placeholder="Select an environment to copy from" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="none">None</SelectItem>
                    {Object.keys(existingEnvironments).map(name => (
                      <SelectItem key={name} value={name}>
                        {name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </CardContent>
            </Card>
          )}

          {/* General Settings */}
          <Card>
            <CardHeader>
              <CardTitle>General</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="name">Environment Name *</Label>
                <Input
                  id="name"
                  value={formData.name}
                  onChange={e =>
                    setFormData({ ...formData, name: e.target.value })
                  }
                  placeholder="e.g., staging, production, dev"
                  required
                />
                <p className="text-xs text-muted-foreground">
                  Must be lowercase-kebab-case (e.g.,
                  &apos;my-environment&apos;)
                </p>
              </div>

              <div className="flex items-center space-x-2">
                <Checkbox
                  id="isDefault"
                  checked={formData.isDefault}
                  onCheckedChange={checked =>
                    setFormData({ ...formData, isDefault: checked as boolean })
                  }
                />
                <Label htmlFor="isDefault" className="cursor-pointer">
                  Set as default environment
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
              Create Environment
            </Button>
          </div>
        </form>
      </div>
    </ErrorBoundary>
  );
}

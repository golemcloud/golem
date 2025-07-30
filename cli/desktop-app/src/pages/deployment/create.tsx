import { useEffect, useMemo, useState } from "react";
import { useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import * as z from "zod";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useToast } from "@/hooks/use-toast";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Loader2, Plus, X } from "lucide-react";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import ErrorBoundary from "@/components/errorBoundary";
import { API } from "@/service";
import { useNavigate, useParams } from "react-router-dom";
import { Card, CardContent } from "@/components/ui/card";

// Define API definition type
interface ApiDefinition {
  id: string;
  versions: string[];
}

const formSchema = z.object({
  domain: z
    .string()
    .min(1, "Domain is required")
    .regex(
      /^localhost(:\d{1,5})?$/,
      "Please enter a valid localhost domain (e.g., localhost:9006)",
    )
    .refine(
      value => {
        if (value.includes(":")) {
          const port = parseInt(value.split(":")[1]!, 10);
          return port >= 1 && port <= 65535;
        }
        return true;
      },
      { message: "Port number must be between 1 and 65535" },
    )
    .transform(value => value.toLowerCase())
    .refine(
      value => !value.startsWith("http://") && !value.startsWith("https://"),
      { message: "Do not include http:// or https://" },
    )
    .refine(
      value => {
        if (!value.includes(":")) {
          return false;
        }
        return true;
      },
      { message: "Port number is required (e.g., localhost:9006)" },
    ),
  definitions: z
    .array(
      z.object({
        id: z.string().min(1, "API definition is required"),
        version: z.string().min(1, "Version is required"),
      }),
    )
    .min(1, "At least one API definition is required")
    .refine(
      definitions => {
        const ids = definitions.map(d => d.id);
        return new Set(ids).size === ids.length;
      },
      { message: "Each API can only be added once" },
    ),
});

type FormValues = z.infer<typeof formSchema>;

export default function CreateDeployment() {
  const [apiDefinitions, setApiDefinitions] = useState<ApiDefinition[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const navigate = useNavigate();
  const { toast } = useToast();
  const { appId } = useParams<{ appId: string }>();

  const form = useForm<FormValues>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      domain: "localhost:9006",
      definitions: [{ id: "", version: "" }],
    },
  });

  const { fields, append, remove } = useFieldArray({
    control: form.control,
    name: "definitions",
  });

  // Fetch API definitions with retry logic
  useEffect(() => {
    const fetchApiDefinitions = async (retryCount = 0) => {
      try {
        setIsLoading(true);
        setFetchError(null);
        const response = await API.apiService.getApiList(appId!);
        const transformedData = Object.values(
          response.reduce(
            (acc, api) => {
              if (api.id && !acc[api.id]) {
                acc[api.id] = { id: api.id, versions: [] };
              }
              if (api.id) {
                acc[api.id]!.versions.push(api.version);
                acc[api.id]!.versions.sort((a, b) =>
                  b.localeCompare(a, undefined, { numeric: true }),
                );
              }
              return acc;
            },
            {} as Record<string, ApiDefinition>,
          ),
        ).sort((a, b) => a.id.localeCompare(b.id));

        setApiDefinitions(transformedData);
      } catch (error) {
        console.error("Failed to fetch API definitions:", error);
        setFetchError("Failed to load API definitions. Please try again.");

        if (retryCount < 3) {
          setTimeout(
            () => fetchApiDefinitions(retryCount + 1),
            1000 * (retryCount + 1),
          );
        }
      } finally {
        setIsLoading(false);
      }
    };

    fetchApiDefinitions();
  }, []);

  const getVersionsForApi = useMemo(() => {
    return (apiId: string) =>
      apiDefinitions.find(api => api.id === apiId)?.versions || [];
  }, [apiDefinitions]);

  const onSubmit = async (data: FormValues) => {
    try {
      setIsSubmitting(true);
      const payload = {
        site: {
          host: data.domain,
          subdomain: null,
        },
        apiDefinitions: data.definitions,
      };
      await API.deploymentService.createDeployment(appId!, payload.site.host);
      toast({
        title: "Deployment was successful",
        duration: 3000,
      });
      navigate(`/app/${appId}/deployments`);
    } catch (error) {
      console.error("Failed to create deployment:", error);
      form.setError("root", {
        type: "manual",
        message: "Failed to create deployment. Please try again.",
      });
    } finally {
      setIsSubmitting(false);
    }
  };

  // Show error state if API fetch fails
  if (fetchError) {
    return (
      <div className="p-6 max-w-3xl mx-auto">
        <div className="flex flex-col items-center justify-center space-y-4 p-8 border rounded-lg bg-destructive/10">
          <p className="text-destructive font-medium">{fetchError}</p>
          <Button variant="outline" onClick={() => window.location.reload()}>
            Retry
          </Button>
        </div>
      </div>
    );
  }

  return (
    <ErrorBoundary>
      <div className="p-6 max-w-3xl mx-auto">
        <Card>
          <CardContent className="p-6 space-y-6">
            <h1 className="text-3xl font-semibold">Deploy API</h1>
            <p className="text-muted-foreground">
              Create a new deployment with one or more API definitions
            </p>
            <Form {...form}>
              <form
                onSubmit={form.handleSubmit(onSubmit)}
                className="space-y-6"
              >
                <FormField
                  control={form.control}
                  name="domain"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Local Domain</FormLabel>
                      <FormControl>
                        <Input
                          placeholder="localhost:9006"
                          {...field}
                          onChange={e => {
                            // Remove any http/https if user pastes them
                            const value = e.target.value
                              .replace(/^https?:\/\//, "")
                              .toLowerCase();
                            field.onChange(value);
                          }}
                        />
                      </FormControl>
                      <FormDescription className="text-[11px] text-muted-foreground">
                        Enter localhost with a port number (e.g.,
                        localhost:9006). The port must be between 1 and 65535.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <div className="space-y-4">
                  <div className="flex items-center justify-between">
                    <div className="space-y-1">
                      <h2 className="text-base font-medium">API Definitions</h2>
                      <p className="text-[11px] text-muted-foreground">
                        Select the APIs and their versions to deploy. Each API
                        can only be added once.
                      </p>
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        // Check if there are any empty definitions before adding new one
                        const hasEmptyDefinition = form
                          .getValues("definitions")
                          .some(def => !def.id || !def.version);
                        if (!hasEmptyDefinition) {
                          append({ id: "", version: "" });
                        }
                      }}
                      disabled={
                        isLoading ||
                        form
                          .getValues("definitions")
                          .some(def => !def.id || !def.version)
                      }
                    >
                      <Plus className="h-4 w-4 mr-2" />
                      Add API
                    </Button>
                  </div>

                  {isLoading ? (
                    <div className="flex items-center justify-center py-8">
                      <Loader2 className="h-6 w-6 animate-spin" />
                      <span className="ml-2">Loading API definitions...</span>
                    </div>
                  ) : (
                    <div className="space-y-4">
                      {fields.map((field, index) => (
                        <div
                          key={field.id}
                          className="grid gap-4 items-start md:grid-cols-[1fr,1fr,auto] p-4 border rounded-lg"
                        >
                          <FormField
                            control={form.control}
                            name={`definitions.${index}.id`}
                            render={({ field }) => (
                              <FormItem>
                                <FormLabel className="text-base font-medium">
                                  API Definition
                                </FormLabel>
                                <Select
                                  onValueChange={value => {
                                    field.onChange(value);
                                    form.setValue(
                                      `definitions.${index}.version`,
                                      "",
                                    );
                                  }}
                                  value={field.value}
                                >
                                  <FormControl>
                                    <SelectTrigger>
                                      <SelectValue placeholder="Select API" />
                                    </SelectTrigger>
                                  </FormControl>
                                  <SelectContent>
                                    {apiDefinitions.map(api => (
                                      <SelectItem key={api.id} value={api.id}>
                                        {api.id}
                                      </SelectItem>
                                    ))}
                                  </SelectContent>
                                </Select>
                                <FormMessage />
                              </FormItem>
                            )}
                          />

                          <FormField
                            control={form.control}
                            name={`definitions.${index}.version`}
                            render={({ field }) => (
                              <FormItem>
                                <FormLabel className="text-base font-medium">
                                  Version
                                </FormLabel>
                                <Select
                                  onValueChange={field.onChange}
                                  value={field.value}
                                  disabled={
                                    !form.watch(`definitions.${index}.id`)
                                  }
                                >
                                  <FormControl>
                                    <SelectTrigger>
                                      <SelectValue placeholder="Select version" />
                                    </SelectTrigger>
                                  </FormControl>
                                  <SelectContent>
                                    {getVersionsForApi(
                                      form.watch(`definitions.${index}.id`),
                                    ).map(version => (
                                      <SelectItem key={version} value={version}>
                                        {version}
                                      </SelectItem>
                                    ))}
                                  </SelectContent>
                                </Select>
                                <FormMessage />
                              </FormItem>
                            )}
                          />

                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            disabled={fields.length === 1}
                            className="mt-8 bg-destructive/20 hover:bg-destructive/50"
                            onClick={() => remove(index)}
                          >
                            <X className="h-4 w-4" />
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                <div className="flex justify-end">
                  <Button type="submit" disabled={isSubmitting || isLoading}>
                    {isSubmitting ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        Deploying...
                      </>
                    ) : (
                      "Deploy"
                    )}
                  </Button>
                </div>
              </form>
            </Form>
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
}

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Info, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

import ErrorBoundary from "@/components/errorBoundary";
import { RibEditor } from "@/components/rib-editor.tsx";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { toast } from "@/hooks/use-toast";
import { parseTypeForTooltip } from "@/lib/utils.ts";
import { API } from "@/service";
import type { GatewayBindingType, MethodPattern } from "@/types/api";
import { Api } from "@/types/api";
import type { Component, ComponentList } from "@/types/component";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import * as z from "zod";

const MethodPattern = z.enum([
  "Get",
  "Post",
  "Put",
  "Delete",
  "Patch",
  "Head",
  "Options",
  "Trace",
  "Connect",
]);

const BindingType = z.enum(["default", "file-server", "cors-preflight"]);

const GatewayBindingData = z.object({
  bindingType: BindingType,
  componentId: z
    .object({
      componentId: z.string(),
      version: z.number(),
    })
    .optional(),
  workerName: z.string().optional(),
  idempotencyKey: z.string().optional(),
  response: z.string().optional(),
});

const HttpCors = z.object({
  allowOrigin: z.string(),
  allowMethods: z.string(),
  allowHeaders: z.string(),
  exposeHeaders: z.string().optional(),
  maxAge: z.number().optional(),
  allowCredentials: z.boolean().optional(),
});

const RouteRequestData = z.object({
  method: MethodPattern,
  path: z.string(),
  binding: GatewayBindingData,
  cors: HttpCors.optional(),
  security: z.string().optional(),
});

function filterMethod(type: string) {
  if (type === "default") {
    return ["Get", "Post", "Put", "Delete", "Patch"];
  } else if (type === "cors-preflight") {
    return ["Options", "Head", "Trace", "Connect"];
  }
  return [];
}

type RouteFormValues = z.infer<typeof RouteRequestData>;

const interpolations = [
  { label: "Path Parameters", expression: "${request.path.<PATH_PARAM_NAME>}" },
  {
    label: "Query Parameters",
    expression: "${request.path.<QUERY_PARAM_NAME>}",
  },
  { label: "Request Body", expression: "${request.body}" },
  { label: "Request Body Field", expression: "${request.body.<FIELD_NAME>}" },
  { label: "Request Headers", expression: "${request.header.<HEADER_NAME>}" },
];

const CreateRoute = () => {
  const { apiName, version } = useParams();
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const [isLoading, setIsLoading] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const reload = queryParams.get("reload");

  const [isEdit, setIsEdit] = useState(false);
  const [activeApiDetails, setActiveApiDetails] = useState<Api | null>(null);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [isPopoverOpen, setIsPopoverOpen] = useState(false);
  const [responseSuggestions, setResponseSuggestions] = useState(
    [] as string[],
  );
  const [variableSuggestions, setVariableSuggestions] = useState(
    {} as Record<string, any>,
  );

  const extractDynamicParams = (path: string) => {
    const pathParamRegex = /{([^}]+)}/g; // Matches {param} in path
    const queryParamRegex = /[?&]([^=]+)={([^}]+)}/g; // Matches ?key={param} or &key={param}

    const pathParams: Record<string, string> = {};
    const queryParams: Record<string, string> = {};
    let match;

    // Extract path parameters
    while ((match = pathParamRegex.exec(path)) !== null) {
      pathParams[match[1]] = match[1];
    }

    // Extract query parameters (key-value pair)
    while ((match = queryParamRegex.exec(path)) !== null) {
      queryParams[match[1]] = match[2]; // key -> param
    }
    setVariableSuggestions({
      path: pathParams,
      query: queryParams,
    });
  };

  const form = useForm<RouteFormValues>({
    resolver: zodResolver(RouteRequestData),
    defaultValues: {
      path: "/",
      method: "Get",
      binding: {
        bindingType: "default",
        componentId: {
          componentId: "",
          version: 0,
        },
        workerName: "",
        response: "",
      },
    },
  });
  // Fetch API details
  useEffect(() => {
    const fetchData = async () => {
      if (!apiName) return;
      try {
        setIsLoading(true);
        const [apiResponse, componentResponse] = await Promise.all([
          API.getApi(apiName),
          API.getComponentByIdAsKey(),
        ]);
        const selectedApi = apiResponse.find(api => api.version === version);
        setActiveApiDetails(selectedApi!);
        setComponentList(componentResponse);
        if (path && method) {
          setIsEdit(true);
          const route = selectedApi?.routes.find(
            route => route.path === path && route.method === method,
          );
          if (route) {
            // Manually set form values instead of using form.reset()
            form.setValue("path", route.path);
            if (route.path) {
              extractDynamicParams(path);
            }
            form.setValue("method", route.method);
            form.setValue(
              "binding.bindingType",
              route.binding.bindingType || "default",
            );
            const componentId = route.binding.componentId?.componentId;
            const versionId = route.binding.componentId?.version;
            if (componentId && versionId) {
              loadResponseSuggestions(
                componentId,
                String(versionId),
                componentResponse,
              );
              form.setValue(
                "binding.componentId.componentId",
                route.binding.componentId?.componentId || "",
              );
              form.setValue(
                "binding.componentId.version",
                route.binding.componentId?.version || 0,
              );
            }
            form.setValue("binding.workerName", route.binding.workerName || "");
            form.setValue("binding.response", route.binding.response || "");
            if (
              route.binding.corsPreflight &&
              route.binding.bindingType === "cors-preflight"
            ) {
              form.setValue(
                "binding.response",
                JSON.stringify(route.binding.corsPreflight) || "",
              );
            }
            form.setValue(
              "binding.idempotencyKey",
              route.binding.idempotencyKey || "",
            );
            form.setValue("cors", route.cors || undefined);
            form.setValue("security", route.security || "");
          }
        }
      } catch (error) {
        console.error("Failed to fetch data:", error);
        setFetchError("Failed to load required data. Please try again.");
      } finally {
        setIsLoading(false);
      }
    };

    fetchData();
  }, [apiName, version, path, method, form]);

  const onSubmit = async (values: RouteFormValues) => {
    if (!activeApiDetails) return;

    try {
      setIsSubmitting(true);

      const apiResponse = await API.getApi(apiName!);
      const selectedApi = apiResponse.find(api => api.version === version);
      if (!selectedApi) {
        toast({
          title: "API not found",
          description: "Please try again.",
          variant: "destructive",
          duration: Number.POSITIVE_INFINITY,
        });
        return;
      }
      selectedApi.routes = selectedApi.routes.filter(
        route => !(route.path === path && route.method === method),
      );
      selectedApi.routes.push(values);
      await API.putApi(
        activeApiDetails.id,
        activeApiDetails.version,
        selectedApi,
      ).then(() => {
        navigate(
          `/apis/${apiName}/version/${version}/routes?path=${values.path}&method=${values.method}&reload=${!reload}`,
        );
      });
    } catch (error) {
      console.error("Failed to create route:", error);
      form.setError("root", {
        type: "manual",
        message: "Failed to create route. Please try again.",
      });
      setIsSubmitting(false);
    } finally {
      setIsSubmitting(false);
    }
  };
  const handlePathChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    form.setValue("path", value);
    extractDynamicParams(value);
  };

  const loadResponseSuggestions = async (
    componentId: string,
    version: string,
    componentResponse: {
      [key: string]: ComponentList;
    },
  ) => {
    const exportedFunctions = componentResponse?.[componentId]?.versions?.find(
      (data: Component) =>
        data.versionedComponentId?.version?.toString() === version,
    );
    const data = exportedFunctions?.metadata?.exports || [];
    const output = data.flatMap(item =>
      item.functions.map(func => {
        const param = func.parameters
          .map(p => {
            const { short } = parseTypeForTooltip(p.typ);
            return `${p.name}: ${short}`;
          })
          .join(", ");
        return `${item.name}.{${func.name}}(${param})`;
      }),
    );
    setResponseSuggestions(output);
  };

  const onVersionChange = (version: string) => {
    form.setValue("binding.componentId.version", Number(version));
    const componentId = form.getValues("binding.componentId.componentId");
    loadResponseSuggestions(componentId, version, componentList);
  };

  const togglePopover = () => {
    setIsPopoverOpen(prev => !prev);
  };

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
      <div className="overflow-y-auto h-[80vh]">
        <div className="max-w-4xl mx-auto p-8">
          {isLoading ? (
            <div className="flex items-center justify-center py-8">
              <Loader2 className="h-6 w-6 animate-spin" />
              <span className="ml-2">Loading...</span>
            </div>
          ) : (
            <Form {...form}>
              <form
                onSubmit={form.handleSubmit(onSubmit)}
                className="space-y-8"
              >
                <div>
                  <h3 className="text-lg font-medium">HTTP Endpoint</h3>
                  <FormDescription>
                    Each API Route must have a unique Method + Path combination.
                  </FormDescription>
                  <div className="space-y-4 mt-4">
                    <FormField
                      control={form.control}
                      name="binding.bindingType"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel required>Bind type</FormLabel>
                          <Select
                            onValueChange={v =>
                              form.setValue(
                                "binding.bindingType",
                                v as GatewayBindingType,
                              )
                            }
                            value={field.value}
                          >
                            <FormControl>
                              <SelectTrigger>
                                <SelectValue placeholder="Select a Binding Type" />
                              </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                              {BindingType.options.map((data: string) => (
                                <SelectItem value={data} key={data}>
                                  {data}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </div>
                  <h3 className="text-lg font-medium pt-10">Worker Binding</h3>
                  <FormDescription>
                    Bind this endpoint to a specific worker function.
                  </FormDescription>
                  <div className="grid grid-cols-2 gap-4 mt-4">
                    <FormField
                      control={form.control}
                      name="binding.componentId.componentId"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel required>Component</FormLabel>
                          <Select
                            onValueChange={componentId => {
                              form.setValue(
                                "binding.componentId.componentId",
                                componentId,
                              );
                              loadResponseSuggestions(
                                componentId,
                                "0",
                                componentList,
                              );
                            }}
                            value={field.value}
                          >
                            <FormControl>
                              <SelectTrigger>
                                <SelectValue placeholder="Select a component" />
                              </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                              {Object.values(componentList).map(
                                (data: ComponentList) => (
                                  <SelectItem
                                    value={data.componentId || ""}
                                    key={data.componentName}
                                  >
                                    {data.componentName}
                                  </SelectItem>
                                ),
                              )}
                            </SelectContent>
                          </Select>
                          <FormMessage />
                        </FormItem>
                      )}
                    />

                    <FormField
                      control={form.control}
                      name="binding.componentId.version"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel required>Version</FormLabel>
                          <Select
                            onValueChange={onVersionChange}
                            value={String(field.value)}
                            disabled={
                              !form.watch("binding.componentId.componentId")
                            }
                          >
                            <FormControl>
                              <SelectTrigger>
                                <SelectValue placeholder="Select version">
                                  {" "}
                                  v{field.value}{" "}
                                </SelectValue>
                              </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                              {form.watch("binding.componentId") &&
                                componentList[
                                  form.watch("binding.componentId.componentId")
                                ]?.versionList?.map((v: number) => (
                                  <SelectItem value={String(v)} key={v}>
                                    v{v}
                                  </SelectItem>
                                ))}
                            </SelectContent>
                          </Select>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </div>
                  {filterMethod(form.watch("binding.bindingType")).length >
                    0 && (
                    <div className="grid grid-cols-3 gap-4 mt-4">
                      <FormField
                        control={form.control}
                        name="method"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel required>Method</FormLabel>
                            <Select
                              onValueChange={v =>
                                form.setValue("method", v as MethodPattern)
                              }
                              value={
                                field.value ||
                                filterMethod(
                                  form.watch("binding.bindingType"),
                                )[0]
                              }
                              disabled={
                                !(
                                  form.watch("binding.bindingType") &&
                                  filterMethod(
                                    form.watch("binding.bindingType"),
                                  ).length > 0
                                )
                              }
                            >
                              <FormControl>
                                <SelectTrigger>
                                  <SelectValue placeholder="Select Method">
                                    {" "}
                                    {field.value}{" "}
                                  </SelectValue>
                                </SelectTrigger>
                              </FormControl>
                              <SelectContent>
                                {form.watch("binding.bindingType") &&
                                  filterMethod(
                                    form.watch("binding.bindingType"),
                                  ).map((v: string) => (
                                    <SelectItem value={v} key={v}>
                                      {v}
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
                        name="path"
                        render={({ field }) => (
                          <FormItem className="col-span-2">
                            <FormLabel required>Path</FormLabel>
                            <FormControl>
                              <Input
                                placeholder="/api/v1/resource/<param>"
                                {...field}
                                onChange={e => handlePathChange(e)}
                              />
                            </FormControl>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                    </div>
                  )}
                </div>

                <div>
                  {form.watch("binding.bindingType") != "cors-preflight" && (
                    <FormField
                      control={form.control}
                      name="binding.workerName"
                      render={({ field }) => (
                        <FormItem className="mt-4">
                          <FormLabel required>Worker Name</FormLabel>
                          <FormControl>
                            <RibEditor
                              {...field}
                              suggestVariable={{ request: variableSuggestions }}
                            />
                          </FormControl>
                          <div>
                            <div className="flex gap-1 items-center">
                              <Popover>
                                <PopoverTrigger asChild>
                                  <button
                                    className="p-1 hover:bg-muted rounded-full transition-colors"
                                    aria-label="Show interpolation info"
                                  >
                                    <Info className="w-4 h-4 text-muted-foreground" />
                                  </button>
                                </PopoverTrigger>
                                <PopoverContent
                                  className="w-[450px] p-4"
                                  align="start"
                                  sideOffset={5}
                                >
                                  <h3 className="text-[13px] font-medium text-card-foreground mb-4 border-b pb-2">
                                    Common Interpolation Expressions
                                  </h3>
                                  <div className="space-y-3">
                                    {interpolations.map(row => (
                                      <div
                                        key={row.label}
                                        className="flex items-center justify-between"
                                      >
                                        <span className="text-[12px] px-2.5 py-0.5 bg-secondary rounded-full text-secondary-foreground font-medium">
                                          {row.label}
                                        </span>
                                        <code className="text-[12px] font-mono text-muted-foreground">
                                          {row.expression}
                                        </code>
                                      </div>
                                    ))}
                                  </div>
                                </PopoverContent>
                              </Popover>
                              <span>
                                Interpolate variables into your Worker ID
                              </span>
                            </div>
                          </div>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  )}
                  <FormField
                    control={form.control}
                    name="binding.response"
                    render={({ field }) => (
                      <FormItem className="mt-4">
                        <FormLabel required>
                          <span className="">
                            Response
                            <Popover
                              open={isPopoverOpen}
                              onOpenChange={setIsPopoverOpen}
                            >
                              <PopoverTrigger asChild>
                                <button
                                  className=" pl-2 hover:bg-muted rounded-full transition-colors"
                                  aria-label="Show interpolation info"
                                  onClick={togglePopover}
                                >
                                  <Info className="w-4 h-4 text-muted-foreground" />
                                </button>
                              </PopoverTrigger>
                              <PopoverContent
                                className={`${
                                  responseSuggestions.length === 0
                                    ? "max-w-[450px]"
                                    : "w-[450px]"
                                }  p-4`}
                                align="start"
                                sideOffset={5}
                              >
                                {responseSuggestions.length > 0 ? (
                                  <div>
                                    <h3 className="text-[13px] font-medium text-card-foreground mb-4 border-b pb-2">
                                      Available Functions
                                    </h3>
                                    <div className="space-y-3 overflow-y-auto max-h-[300px]">
                                      {responseSuggestions.map(row => (
                                        <div
                                          key={row}
                                          className="flex items-center justify-between"
                                          onClick={e => {
                                            e.stopPropagation();
                                            navigator.clipboard.writeText(
                                              `${row} `,
                                            );
                                            toast({
                                              title: "Copied to clipboard",
                                              duration: 3000,
                                            });
                                            setIsPopoverOpen(false);
                                          }}
                                        >
                                          <span className="text-[12px] min-h-[20px] font-mono text-muted-foreground hover:border-b cursor-pointer">
                                            {row}
                                          </span>
                                        </div>
                                      ))}
                                    </div>
                                  </div>
                                ) : (
                                  <div className="text-center text-muted-foreground">
                                    No component version selected
                                  </div>
                                )}
                              </PopoverContent>
                            </Popover>
                          </span>
                        </FormLabel>
                        <FormControl>
                          <RibEditor
                            {...field}
                            scriptKeys={responseSuggestions}
                            suggestVariable={{ request: variableSuggestions }}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
                <div className="flex justify-end space-x-3">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => form.reset()}
                    disabled={isSubmitting}
                  >
                    Clear
                  </Button>
                  <Button type="submit" disabled={isSubmitting}>
                    {isSubmitting ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        {isEdit ? "Editing..." : "Creating..."}
                      </>
                    ) : (
                      <div>{isEdit ? "Edit Route" : "Create Route"}</div>
                    )}
                  </Button>
                </div>
              </form>
            </Form>
          )}
        </div>
      </div>
    </ErrorBoundary>
  );
};

export default CreateRoute;

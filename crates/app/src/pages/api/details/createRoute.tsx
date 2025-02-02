import { useState, useEffect, useRef } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, Loader2 } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from "@/components/ui/form";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import * as z from "zod";
import { API } from "@/service";
import type { Api, HttpMethod } from "@/types/api";
import type { ComponentList } from "@/types/component";
import ErrorBoundary from "@/components/errorBoundary";
import { toast } from "@/hooks/use-toast";
import { Card } from "@/components/ui/card";
import { getCaretCoordinates } from "@/lib/worker";

const extractDynamicParams = (path: string) => {
  const regex = /{([^}]+)}/g;
  const matches = [];
  let match;

  while ((match = regex.exec(path)) !== null) {
    matches.push(match[1]);
  }

  return matches;
};

const HTTP_METHODS = [
  "Get",
  "Post",
  "Put",
  "Patch",
  "Delete",
  "Head",
  "Options",
  "Trace",
  "Connect",
] as const;

const routeSchema = z.object({
  method: z.enum(HTTP_METHODS),
  path: z
    .string()
    .min(1, "Path is required")
    .regex(/^\//, "Path must start with /")
    .regex(
      /^[a-zA-Z0-9/\-_<>{}]+$/,
      "Path can only contain letters, numbers, slashes, hyphens, underscores, and path parameters in <>"
    ),
  componentId: z.string().min(1, "Component is required"),
  version: z.string().min(0, "Version is required"),
  workerName: z
    .string()
    .min(1, "Worker Name is required")
    .max(100, "Worker Name cannot exceed 100 characters"),
  response: z.string().optional(),
});

type RouteFormValues = z.infer<typeof routeSchema>;

const CreateRoute = () => {
  const navigate = useNavigate();
  const { apiName, version } = useParams();
  const [isLoading, setIsLoading] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [componentList, setComponentList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const [isEdit, setIsEdit] = useState(false);
  const [activeApiDetails, setActiveApiDetails] = useState<Api | null>(null);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0 });
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [cursorPosition, setCursorPosition] = useState(0);

  const [responseSuggestions, setResponseSuggestions] = useState(
    [] as string[]
  );
  const [filteredResponseSuggestions, setFilteredResponseSuggestions] =
    useState<string[]>([]);
  const [showResponseSuggestions, setShowResponseSuggestions] = useState(false);
  const [responseMenuPosition, setResponseMenuPosition] = useState({
    top: 0,
    left: 0,
  });
  const responseTextareaRef = useRef<HTMLTextAreaElement>(null);
  const [responseCursorPosition, setResponseCursorPosition] = useState(0);

  const form = useForm<RouteFormValues>({
    resolver: zodResolver(routeSchema),
    defaultValues: {
      method: "Get",
      path: "",
      componentId: "",
      version: "",
      workerName: "",
      response: "",
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
        const selectedApi = apiResponse.find((api) => api.version === version);
        setActiveApiDetails(selectedApi!);
        setComponentList(componentResponse);
        if (path && method) {
          setIsEdit(true);
          const route = selectedApi?.routes.find(
            (route) => route.path === path && route.method === method
          );
          form.setValue("method", (route?.method as HttpMethod) ?? "Get");
          form.setValue("path", route?.path || "");

          form.setValue(
            "componentId",
            route?.binding?.componentId?.componentId || ""
          );
          form.setValue(
            "version",
            String(route?.binding?.componentId?.version ?? "")
          );
          form.setValue("workerName", route?.binding?.workerName || "");
          form.setValue("response", route?.binding?.response || "");
        }
      } catch (error) {
        console.error("Failed to fetch data:", error);
        setFetchError("Failed to load required data. Please try again.");
      } finally {
        setIsLoading(false);
      }
    };

    fetchData();
  }, [apiName, version, path, method]);

  const onSubmit = async (values: RouteFormValues) => {
    if (!activeApiDetails) return;

    try {
      setIsSubmitting(true);

      const apiResponse = await API.getApi(apiName!);
      const selectedApi = apiResponse.find((api) => api.version === version);
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
        (route) => !(route.path === path && route.method === method)
      );
      selectedApi.routes.push({
        method: values.method,
        path: values.path,
        binding: {
          componentId: {
            componentId: values.componentId,
            version: Number.parseInt(values.version),
          },
          workerName: values.workerName,
          response: values.response || "",
        },
      });
      await API.putApi(
        activeApiDetails.id,
        activeApiDetails.version,
        selectedApi
      ).then(() => {
        navigate(
          `/apis/${apiName}/version/${version}/routes?path=${values.path}&method=${values.method}`
        );
      });
    } catch (error) {
      console.error("Failed to create route:", error);
      form.setError("root", {
        type: "manual",
        message: "Failed to create route. Please try again.",
      });
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleSuggestionClick = (suggestion: string) => {
    const currentValue = form.getValues("workerName");
    const textBeforeCursor = currentValue.slice(0, cursorPosition);
    const pattern = "request.path.";
    const startIndex = textBeforeCursor.lastIndexOf(pattern);

    let newValue: string;
    let newCursorPosition: number;

    if (startIndex !== -1) {
      // Replace any text after "request.path." with the suggestion
      newValue =
        currentValue.slice(0, startIndex) +
        pattern +
        suggestion +
        currentValue.slice(cursorPosition);
      newCursorPosition = startIndex + pattern.length + suggestion.length;
    } else {
      // If the pattern isn't found, insert it along with the suggestion at the cursor position
      newValue =
        currentValue.slice(0, cursorPosition) +
        pattern +
        suggestion +
        currentValue.slice(cursorPosition);
      newCursorPosition = cursorPosition + pattern.length + suggestion.length;
    }

    form.setValue("workerName", newValue);
    setShowSuggestions(false);

    if (textareaRef.current) {
      textareaRef.current.focus();
      textareaRef.current.setSelectionRange(
        newCursorPosition,
        newCursorPosition
      );
      setCursorPosition(newCursorPosition);
    }
  };

  const onComponentChange = (componentId: string) => {
    form.setValue("componentId", componentId);
  };

  const onVersionChange = (version: string) => {
    form.setValue("version", version);
    const componentId = form.getValues("componentId");
    const exportedFunctions = componentList?.[componentId]?.versions?.find(
      (data) => data.versionedComponentId?.version?.toString() === version
    );
    const data = exportedFunctions?.metadata?.exports || [];
    const output = data.flatMap((item) =>
      item.functions.map((func) => `${item.name}.{${func.name}}`)
    );
    setResponseSuggestions(output);
  };

  const handleWorkerNameChange = (
    e: React.ChangeEvent<HTMLTextAreaElement>
  ) => {
    const value = e.target.value;
    form.setValue("workerName", value);
    const cursorPos = e.target.selectionStart || 0;
    setCursorPosition(cursorPos);

    const textBeforeCursor = value.slice(0, cursorPos);
    // Look for the last occurrence of "request.path."
    const pattern = "request.path.";
    const startIndex = textBeforeCursor.lastIndexOf(pattern);

    if (startIndex !== -1) {
      // Extract the token typed after "request.path."
      const token = textBeforeCursor.slice(startIndex + pattern.length);
      // Retrieve the dynamic parameters (or suggestion candidates)
      const dynamicParams = extractDynamicParams(form.getValues("path"));

      // If token is empty, show all dynamicParams; otherwise filter them
      const filteredSuggestions =
        token.trim().length > 0
          ? dynamicParams.filter((param) =>
              param.toLowerCase().startsWith(token.toLowerCase())
            )
          : dynamicParams;

      if (filteredSuggestions.length > 0) {
        setSuggestions(filteredSuggestions);
        updateMenuPosition();
        setShowSuggestions(true);
      } else {
        setShowSuggestions(false);
      }
    } else {
      setShowSuggestions(false);
    }
  };

  const updateMenuPosition = () => {
    if (textareaRef.current) {
      const { selectionStart } = textareaRef.current;
      const coords = getCaretCoordinates(textareaRef.current, selectionStart);
      setMenuPosition({
        top: coords.top + coords.height - textareaRef.current.scrollTop,
        left: coords.left - textareaRef.current.scrollLeft,
      });
    }
  };

  const handleResponseSuggestionClick = (suggestion: string) => {
    const currentValue = form.getValues("response") ?? "";
    // Get text before the current cursor position.
    const textBeforeCursor = currentValue.slice(0, responseCursorPosition);
    // Find the last contiguous non-space token
    const tokenMatch = textBeforeCursor.match(/(\S+)$/);
    let tokenStart = responseCursorPosition;
    if (tokenMatch) {
      tokenStart = responseCursorPosition - tokenMatch[1].length;
    }
    // Replace the token with the suggestion.
    const newValue =
      currentValue.slice(0, tokenStart) +
      suggestion +
      currentValue.slice(responseCursorPosition);
    form.setValue("response", newValue);
    setShowResponseSuggestions(false);

    if (responseTextareaRef.current) {
      responseTextareaRef.current.focus();
      const newCursorPosition = tokenStart + suggestion.length;
      responseTextareaRef.current.setSelectionRange(
        newCursorPosition,
        newCursorPosition
      );
      setResponseCursorPosition(newCursorPosition);
    }
  };

  const handleResponseChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.target.value;
    form.setValue("response", value);
    const cursorPos = e.target.selectionStart || 0;
    setResponseCursorPosition(cursorPos);

    // Extract the last "word" (non-whitespace sequence) before the cursor.
    const textBeforeCursor = value.slice(0, cursorPos);
    const match = textBeforeCursor.match(/(\S+)$/); // captures last token
    const token = match ? match[1] : "";

    // Filter responseSuggestions to only those that match the token (case-insensitive)
    const filtered = responseSuggestions.filter((item) =>
      item.toLowerCase().startsWith(token.toLowerCase())
    );

    // If there are any matches and the token is not empty, show the dropdown.
    if (filtered.length > 0 && token.length > 0) {
      updateResponseMenuPosition();
      setFilteredResponseSuggestions(filtered);
      setShowResponseSuggestions(true);
    } else {
      setShowResponseSuggestions(false);
    }
  };

  const updateResponseMenuPosition = () => {
    if (responseTextareaRef.current) {
      const { selectionStart } = responseTextareaRef.current;
      const coords = getCaretCoordinates(
        responseTextareaRef.current,
        selectionStart
      );
      setResponseMenuPosition({
        top: coords.top + coords.height - responseTextareaRef.current.scrollTop,
        left: coords.left - responseTextareaRef.current.scrollLeft,
      });
    }
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
          <div className="flex items-center gap-2 mb-8">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => navigate(`/apis/${apiName}/version/${version}`)}
            >
              <ArrowLeft className="mr-2" />
              Back
            </Button>
            <span className="text-lg font-medium">
              {isEdit ? "Edit Route" : "Create New Route"}
            </span>
          </div>

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
                      name="method"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>Method</FormLabel>
                          <div className="flex flex-wrap gap-2 mt-2">
                            {HTTP_METHODS.map((m) => (
                              <Button
                                type="button"
                                key={m}
                                variant={
                                  field.value === m ? "default" : "outline"
                                }
                                size="sm"
                                onClick={() => field.onChange(m)}
                              >
                                {m}
                              </Button>
                            ))}
                          </div>
                          <FormMessage />
                        </FormItem>
                      )}
                    />

                    <FormField
                      control={form.control}
                      name="path"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>Path</FormLabel>
                          <FormControl>
                            <Input
                              placeholder="/api/v1/resource/<param>"
                              {...field}
                            />
                          </FormControl>
                          <FormDescription>
                            Define path variables with angle brackets (e.g.,
                            /users/id)
                          </FormDescription>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </div>
                </div>

                <div>
                  <h3 className="text-lg font-medium">Worker Binding</h3>
                  <FormDescription>
                    Bind this endpoint to a specific worker function.
                  </FormDescription>
                  <div className="grid grid-cols-2 gap-4 mt-4">
                    <FormField
                      control={form.control}
                      name="componentId"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>Component</FormLabel>
                          <Select
                            onValueChange={onComponentChange}
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
                                )
                              )}
                            </SelectContent>
                          </Select>
                          <FormMessage />
                        </FormItem>
                      )}
                    />

                    <FormField
                      control={form.control}
                      name="version"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>Version</FormLabel>
                          <Select
                            onValueChange={onVersionChange}
                            value={field.value}
                            disabled={!form.watch("componentId")}
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
                              {form.watch("componentId") &&
                                componentList[
                                  form.watch("componentId")
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

                  <FormField
                    control={form.control}
                    name="workerName"
                    render={({ field }) => (
                      <FormItem className="mt-4">
                        <FormLabel>Worker Name</FormLabel>
                        <FormControl>
                          <div className="relative">
                            <Textarea
                              placeholder="Interpolate variables into your Worker ID"
                              {...field}
                              onChange={handleWorkerNameChange}
                              ref={textareaRef}
                            />
                            {showSuggestions && (
                              <Card
                                className="absolute z-10 p-1 space-y-1 bg-white shadow-lg min-w-[70px]"
                                style={{
                                  top: `${menuPosition.top}px`,
                                  left: `${menuPosition.left}px`,
                                  width: "max-content",
                                }}
                              >
                                {suggestions.map((suggestion) => (
                                  <div
                                    key={suggestion}
                                    className="px-2 py-1 text-sm cursor-pointer hover:bg-gray-100"
                                    onClick={() =>
                                      handleSuggestionClick(suggestion)
                                    }
                                  >
                                    {suggestion}
                                  </div>
                                ))}
                              </Card>
                            )}
                          </div>
                        </FormControl>
                        <FormDescription>
                          Unique identifier for your worker instance. Use ${"{"}{" "}
                          to interpolate path parameters.
                        </FormDescription>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>

                <FormField
                  control={form.control}
                  name="response"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Response</FormLabel>
                      <FormControl>
                        <div className="relative">
                          <Textarea
                            placeholder="Define the HTTP response template"
                            className="min-h-[130px]"
                            {...field}
                            onChange={handleResponseChange}
                            ref={responseTextareaRef}
                          />
                          {showResponseSuggestions && (
                            <Card
                              className="absolute z-10 p-1 space-y-1 bg-white shadow-lg min-w-[200px]"
                              style={{
                                top: `${responseMenuPosition.top}px`,
                                left: `${responseMenuPosition.left}px`,
                                width: "max-content",
                              }}
                            >
                              {filteredResponseSuggestions.map((suggestion) => (
                                <div
                                  key={suggestion}
                                  className="px-2 py-1 text-sm cursor-pointer hover:bg-gray-100"
                                  onClick={() =>
                                    handleResponseSuggestionClick(suggestion)
                                  }
                                >
                                  {suggestion}
                                </div>
                              ))}
                            </Card>
                          )}
                        </div>
                      </FormControl>
                      <FormDescription>
                        Type 'golem:' to see available functions. Define the
                        HTTP response for this API Route.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

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
                        Creating...
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

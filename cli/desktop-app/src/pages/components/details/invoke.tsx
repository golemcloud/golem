import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { API } from "@/service";
import {
  ComponentExportFunction,
  ComponentList,
  Export,
} from "@/types/component.ts";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { CodeBlock, dracula } from "react-code-blocks";
import {
  ClipboardCopy,
  Play,
  Presentation,
  TableIcon,
  TimerReset,
  Info,
  Check,
} from "lucide-react";
import { cn, sanitizeInput } from "@/lib/utils";
import ReactJson from "react-json-view";
import { useTheme } from "@/components/theme-provider.tsx";
import { Textarea } from "@/components/ui/textarea";
import {
  parseToJsonEditor,
  parseTypesData,
  safeFormatJSON,
  parseTooltipTypesData,
  validateJsonStructure,
} from "@/lib/worker";
import { toast } from "@/hooks/use-toast";
import {
  DynamicForm,
  nonStringPrimitives,
} from "@/pages/workers/details/dynamic-form.tsx";

export default function ComponentInvoke() {
  const { componentId = "" } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const name = searchParams.get("name") || "";
  const urlFn = searchParams.get("fn") || "";

  const [functionDetails, setFunctionDetails] =
    useState<ComponentExportFunction | null>(null);
  const [value, setValue] = useState<string>("{}");
  const [resultValue, setResultValue] = useState<string>("");
  const [componentList, setComponentList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const [viewMode, setViewMode] = useState("form");

  /** Fetch function details based on URL params. */
  const fetchFunctionDetails = useCallback(async () => {
    try {
      const data = await API.getComponentByIdAsKey();
      setComponentList(data);
      const matchingComponent =
        data?.[componentId].versions?.[data?.[componentId].versions.length - 1];
      if (!matchingComponent) {
        throw new Error("Component not found.");
      }
      if (name && urlFn) {
        const exportItem = matchingComponent.metadata?.exports?.find(
          (e: Export) => e.name === name,
        );
        if (!exportItem) {
          throw new Error("Export item not found.");
        }

        const fnDetails = exportItem.functions?.find(
          (f: ComponentExportFunction) => f.name === urlFn,
        );
        if (!fnDetails) {
          throw new Error("Function details not found.");
        }
        setFunctionDetails(fnDetails);
        const initialJson = parseToJsonEditor(fnDetails);
        // Pre-format the JSON so it looks nice in the textarea
        setValue(JSON.stringify(initialJson, null, 2));
      } else if (
        !name &&
        !urlFn &&
        matchingComponent?.metadata?.exports?.[0]?.functions?.[0]
      ) {
        navigate(
          `/components/${componentId}/invoke?name=${matchingComponent.metadata.exports[0].name}&&fn=${matchingComponent.metadata.exports[0].functions[0].name}`,
        );
      }
    } catch (error: unknown) {
      if (error instanceof Error) {
        toast({
          title: error.message,
          variant: "destructive",
          duration: Number.POSITIVE_INFINITY,
        });
      } else {
        toast({
          title: "Unable to fetch function details.",
          variant: "destructive",
          duration: Number.POSITIVE_INFINITY,
        });
      }
    }
  }, [componentId, urlFn, name]);

  useEffect(() => {
    if (componentId) {
      setResultValue("");
      fetchFunctionDetails();
    }
  }, [componentId, name, urlFn, fetchFunctionDetails]);

  const handleValueChange = (newValue: string) => {
    const formatted = safeFormatJSON(newValue);
    setValue(formatted);
    setResultValue("");
  };

  const onInvoke = async (parsedValue: unknown[]) => {
    try {
      if (!functionDetails) {
        throw new Error("No function details loaded.");
      }

      const typeData = parseTypesData(functionDetails);

      const params: { value: unknown; typ: unknown }[] = [];
      parsedValue.map((value, index) => {
        params.push({
          value: value,
          typ: typeData.typ.items[index],
        });
      });

      const functionName = `${encodeURIComponent(name)}.${encodeURIComponent(
        `{${urlFn}}`,
      )}`;
      const response = await API.invokeEphemeralAwait(
        componentId,
        functionName,
        { params },
      );

      const newValue = JSON.stringify(response?.result?.value, null, 2);
      setResultValue(newValue);
    } catch (error: unknown) {
      if (
        typeof error === "object" &&
        error !== null &&
        "description" in error
      ) {
        const description = (error as { description?: string }).description;
        toast({
          title: description ?? "An unknown error occurred.",
          variant: "destructive",
        });
      } else if (typeof error === "string" || typeof error === "object") {
        toast({
          title: String(error),
          variant: "destructive",
        });
      }
    }
  };

  const copyToClipboard = () => {
    navigator.clipboard.writeText(value);
  };

  const componentDetails =
    componentList[componentId]?.versions?.[
      componentList[componentId]?.versions.length - 1
    ] || {};

  return (
    <div className="flex">
      <div className="flex-1 flex flex-col bg-background">
        <div className="flex">
          <div className="border-r px-8 py-4 min-w-[300px]">
            <div className="flex flex-col gap-4 overflow-scroll h-[85vh]">
              {componentDetails?.metadata?.exports?.map(exportItem => (
                <div key={exportItem.name}>
                  <div className="flex items-center justify-between">
                    <span className="text-sm text-neutral-600 font-bold pb-4">
                      {exportItem.name}
                    </span>
                  </div>
                  <ul className="space-y-1">
                    {exportItem?.functions?.length > 0 &&
                      exportItem.functions.map(
                        (fn: ComponentExportFunction) => (
                          <li key={fn.name}>
                            <Button
                              variant="ghost"
                              onClick={() =>
                                navigate(
                                  `/components/${componentId}/invoke?name=${exportItem.name}&&fn=${fn.name}`,
                                )
                              }
                              className={cn(
                                "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                                urlFn === fn.name
                                  ? "bg-gray-300 dark:bg-neutral-800 text-gray-900 dark:text-gray-100"
                                  : "hover:bg-gray-200 dark:hover:bg-neutral-700 text-gray-700 dark:text-gray-300",
                              )}
                            >
                              <span>{fn.name}</span>
                            </Button>
                          </li>
                        ),
                      )}
                  </ul>
                </div>
              ))}
            </div>
          </div>
          <div className="flex-1">
            <header className="w-full border-b py-4 px-6">
              <h3>
                {name} - {urlFn}
              </h3>
            </header>

            <div className="p-10 space-y-6 mx-auto overflow-auto h-[80vh]">
              <main className="flex-1 space-y-6">
                <header className="flex gap-4 items-center mb-4">
                  <div className="flex-1 flex items-center gap-2">
                    <Button
                      variant="outline"
                      onClick={() => {
                        setResultValue("");
                        setViewMode("form");
                      }}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "form"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <ClipboardCopy className="h-4 w-4 mr-1" />
                      Form Layout
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => {
                        setResultValue("");
                        setViewMode("preview");
                      }}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "preview"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <Presentation className="h-4 w-4 mr-1" />
                      Json Layout
                    </Button>
                  </div>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      onClick={() => setViewMode("types")}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "types"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <TableIcon className="h-4 w-4 mr-1" />
                      Types
                    </Button>
                  </div>
                </header>
                {viewMode === "form" && functionDetails && (
                  <DynamicForm
                    functionDetails={functionDetails}
                    onInvoke={data => onInvoke(data)}
                    resetResult={() => setResultValue("")}
                  />
                )}
                {viewMode === "preview" && functionDetails && (
                  <SectionCard
                    title="Preview"
                    description="Preview the current function invocation arguments"
                    value={value}
                    onValueChange={handleValueChange}
                    copyToClipboard={copyToClipboard}
                    functionDetails={functionDetails}
                    onReset={() => {
                      if (functionDetails) {
                        const initialJson = parseToJsonEditor(functionDetails);
                        setValue(JSON.stringify(initialJson, null, 2));
                      }
                    }}
                    onInvoke={onInvoke}
                  />
                )}

                {viewMode === "types" && functionDetails && (
                  <SectionCard
                    title="Types"
                    description="Types of the function arguments"
                    value={JSON.stringify(
                      parseTypesData(functionDetails),
                      null,
                      2,
                    )}
                    functionDetails={functionDetails}
                    copyToClipboard={() => {
                      navigator.clipboard.writeText(
                        JSON.stringify(
                          parseTypesData(functionDetails),
                          null,
                          2,
                        ),
                      );
                    }}
                    readOnly={true}
                  />
                )}
                {resultValue && functionDetails && (
                  <SectionCard
                    title="Result"
                    description="View the result of your latest worker invocation"
                    value={resultValue}
                    readOnly={true}
                    functionDetails={functionDetails}
                  />
                )}
              </main>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ----------------------------------
 * Reusable SectionCard Component
 * ---------------------------------- */

interface SectionCardProps {
  title: string;
  description: string;
  value: string;
  onValueChange?: (value: string) => void;
  copyToClipboard?: () => void;
  readOnly?: boolean;
  functionDetails?: ComponentExportFunction;
  onInvoke?: (args: unknown[]) => void;
  onReset?: () => void;
}

function SectionCard({
  title,
  description,
  value,
  onValueChange,
  copyToClipboard,
  functionDetails,
  readOnly = false,
  onInvoke = () => {},
  onReset = () => {},
}: SectionCardProps) {
  const { theme } = useTheme();
  const [copied, setCopied] = useState(false);
  const [errors, setErrors] = useState<Record<string, string>>({});

  const handleCopy = () => {
    if (copyToClipboard) {
      copyToClipboard();
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const onSubmit = () => {
    try {
      const sanitizedValue = sanitizeInput(value);
      const parsedValue = JSON.parse(sanitizedValue);
      const validationErrors = validateForm(parsedValue);
      if (Object.keys(validationErrors).length > 0) {
        setErrors(validationErrors);
      } else {
        onInvoke(parsedValue);
      }
    } catch (error) {
      setErrors({ root: "Invalid JSON format" });
    }
  };

  const validateForm = (parsedValue: any[]): Record<string, string> => {
    const validationErrors: Record<string, string> = {};
    if (!functionDetails) {
      throw new Error("No function details loaded.");
    }
    functionDetails.parameters.forEach((field, index) => {
      let value = parsedValue[index];
      if (nonStringPrimitives.includes(field.typ.type) && value === undefined) {
        validationErrors[field.name] = `${field.name} is required`;
      } else {
        const error = validateJsonStructure(value, field);
        if (error) {
          validationErrors[field.name] = error;
        }
      }
    });
    return validationErrors;
  };

  return (
    <div>
      <Card className="w-full bg-background">
        <CardHeader className="flex items-center pb-2 flex-row">
          <div className="flex items-center justify-between w-full">
            <div>
              <CardTitle className="text-xl font-bold flex items-center gap-4">
                <div>{title}</div>
                {!readOnly && functionDetails && (
                  <Popover>
                    <PopoverTrigger asChild>
                      <button
                        className="p-1 hover:bg-muted rounded-full transition-colors"
                        aria-label="Show interpolation info"
                      >
                        <Info className="w-4 h-4 text-muted-foreground" />
                      </button>
                    </PopoverTrigger>
                    <PopoverContent className="w-[500px] p-2 text-[13px] bg-gray-800 text-white rounded-lg shadow-lg max-h-[500px] overflow-scroll">
                      <CodeBlock
                        text={JSON.stringify(
                          parseTooltipTypesData(functionDetails),
                          null,
                          2,
                        )}
                        language="json"
                        theme={dracula}
                      />
                    </PopoverContent>
                  </Popover>
                )}
              </CardTitle>
              <p className="text-sm text-muted-foreground">{description}</p>
            </div>
            {copyToClipboard && (
              <Button variant="outline" size="sm" onClick={handleCopy}>
                {copied ? (
                  <>
                    <Check className="h-4 w-4 mr-1 text-green-500" />
                    Copied!
                  </>
                ) : (
                  <>
                    <ClipboardCopy className="h-4 w-4 mr-1" />
                    Copy
                  </>
                )}
              </Button>
            )}
          </div>
        </CardHeader>
        <CardContent>
          {readOnly ? (
            <ReactJson
              src={JSON.parse(value || "{}")}
              name={null}
              theme={theme == "dark" ? "brewer" : "bright:inverted"}
              collapsed={false}
              enableClipboard={false}
              displayDataTypes={false}
              style={{ fontSize: "14px", lineHeight: "1.6" }}
            />
          ) : (
            <Textarea
              value={value}
              onChange={e => {
                setErrors({});
                onValueChange?.(e.target.value);
              }}
              className={`min-h-[400px] font-mono text-sm mt-2 ${Object.keys(errors).length > 0 ? "border-red-500" : ""}`}
              placeholder="Enter JSON data..."
            />
          )}
          {Object.keys(errors).length > 0 && (
            <div className="text-red-500 text-sm mt-2">
              {Object.values(errors).join(", ")}
            </div>
          )}
        </CardContent>
      </Card>
      {!readOnly && (
        <div className="flex gap-4 justify-end mt-4">
          <Button
            variant="outline"
            onClick={() => {
              onReset();
              setErrors({});
            }}
            className="text-primary hover:bg-primary/10 hover:text-primary"
          >
            <TimerReset className="h-4 w-4 mr-1" />
            Reset
          </Button>
          <Button onClick={onSubmit}>
            <Play className="h-4 w-4 mr-1" />
            Invoke
          </Button>
        </div>
      )}
    </div>
  );
}

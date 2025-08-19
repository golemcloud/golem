import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { CodeBlock, dracula } from "react-code-blocks";
import { Check, ClipboardCopy, Info, Play, TimerReset } from "lucide-react";
import ReactJson from "@microlink/react-json-view";
import { Textarea } from "@/components/ui/textarea";
import { useTheme } from "@/components/theme-provider.tsx";
import { ComponentExportFunction } from "@/types/component.ts";
import { sanitizeInput } from "@/lib/utils";
import { parseTooltipTypesData, RawTypesInput } from "@/lib/worker";
import {
  isHttpHandlerFunction,
  canInvokeHttpHandler,
} from "@/lib/http-handler";
import { InvokeParams } from "./InvokeLayout";

export interface SectionCardProps {
  title: string;
  description: string;
  value: string;
  onValueChange?: (value: string) => void;
  copyToClipboard?: () => void;
  readOnly?: boolean;
  functionDetails?: ComponentExportFunction;
  onInvoke?: (args: InvokeParams) => void;
  onReset?: () => void;
  exportName?: string;
  functionName?: string;
}

export function SectionCard({
  title,
  description,
  value,
  onValueChange,
  copyToClipboard,
  functionDetails,
  readOnly = false,
  onInvoke = () => {},
  onReset = () => {},
  exportName = "",
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

      // Check if HTTP handler can be invoked directly
      const canInvoke = canInvokeHttpHandler(exportName);

      if (!canInvoke) {
        setErrors({
          root: "This HTTP handler cannot be invoked directly via CLI. It should be triggered by HTTP requests.",
        });
        return;
      }

      // Special handling for HTTP incoming handlers - accept any valid JSON
      const isHttpHandler = isHttpHandlerFunction(exportName);

      if (isHttpHandler) {
        // For HTTP handlers, we're more permissive with validation
        onInvoke(parsedValue);
        return;
      }

      // Let CLI handle validation for other function types
      onInvoke(parsedValue);
    } catch {
      setErrors({ root: "Invalid JSON format" });
    }
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
                          parseTooltipTypesData(
                            functionDetails as RawTypesInput,
                          ),
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
          {/* Warning for HTTP handlers */}
          {!readOnly && !canInvokeHttpHandler(exportName) && (
            <div className="mb-4 p-4 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg">
              <div className="flex items-start">
                <Info className="w-5 h-5 text-yellow-600 dark:text-yellow-400 mt-0.5 mr-3 flex-shrink-0" />
                <div>
                  <h4 className="text-sm font-medium text-yellow-800 dark:text-yellow-200">
                    Cannot invoke HTTP handler directly
                  </h4>
                  <p className="text-sm text-yellow-700 dark:text-yellow-300 mt-1">
                    This is an HTTP incoming handler that is designed to be
                    triggered by incoming HTTP requests, not direct CLI
                    invocation.
                  </p>
                </div>
              </div>
            </div>
          )}

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
              className={`min-h-[400px] font-mono text-sm mt-2 ${
                Object.keys(errors).length > 0 ? "border-red-500" : ""
              }`}
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

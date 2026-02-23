import { useCallback, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { HTTP_METHOD_COLOR } from "@/components/nav-route";
import ReactJson from "@microlink/react-json-view";
import { useTheme } from "@/components/theme-provider";
import { cn } from "@/lib/utils";
import { Loader2, Plus, Trash2 } from "lucide-react";

const lightJsonTheme = {
  base00: "#ffffff",
  base01: "#f5f5f5",
  base02: "#d0d0d0",
  base03: "#b0b0b0",
  base04: "#505050",
  base05: "#303030",
  base06: "#1a1a1a",
  base07: "#000000",
  base08: "#d73a49",
  base09: "#e36209",
  base0A: "#b08800",
  base0B: "#22863a",
  base0C: "#1b7c83",
  base0D: "#005cc5",
  base0E: "#6f42c1",
  base0F: "#cb2431",
};

interface HeaderEntry {
  key: string;
  value: string;
}

interface ApiResponse {
  status: number;
  statusText: string;
  body: string;
  timeMs: number;
  isJson: boolean;
}

interface ApiTesterModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  method: string;
  path: string;
  host: string;
}

const METHODS_WITH_BODY = ["POST", "PUT", "PATCH"];

export function ApiTesterModal({
  open,
  onOpenChange,
  method,
  path,
  host,
}: ApiTesterModalProps) {
  const { resolvedTheme } = useTheme();

  // Extract path parameters from {param} patterns
  const pathParams = useMemo(() => {
    const matches = path.match(/\{([^}]+)\}/g);
    if (!matches) return [];
    return matches.map(m => m.slice(1, -1));
  }, [path]);

  const [paramValues, setParamValues] = useState<Record<string, string>>({});
  const [headers, setHeaders] = useState<HeaderEntry[]>([
    { key: "Content-Type", value: "application/json" },
    { key: "Accept", value: "application/json" },
  ]);
  const [body, setBody] = useState("{}");
  const [loading, setLoading] = useState(false);
  const [response, setResponse] = useState<ApiResponse | null>(null);

  const upperMethod = method.toUpperCase();
  const hasBody = METHODS_WITH_BODY.includes(upperMethod);

  const resolvedPath = useMemo(() => {
    let resolved = path;
    for (const param of pathParams) {
      const value = paramValues[param] || `{${param}}`;
      resolved = resolved.replace(`{${param}}`, value);
    }
    return resolved;
  }, [path, pathParams, paramValues]);

  const fullUrl = `http://${host}${resolvedPath}`;

  const addHeader = useCallback(() => {
    setHeaders(prev => [...prev, { key: "", value: "" }]);
  }, []);

  const removeHeader = useCallback((index: number) => {
    setHeaders(prev => prev.filter((_, i) => i !== index));
  }, []);

  const updateHeader = useCallback(
    (index: number, field: "key" | "value", val: string) => {
      setHeaders(prev =>
        prev.map((h, i) => (i === index ? { ...h, [field]: val } : h)),
      );
    },
    [],
  );

  const sendRequest = async () => {
    setLoading(true);
    setResponse(null);

    const headersObj: Record<string, string> = {};
    for (const h of headers) {
      if (h.key.trim()) {
        headersObj[h.key.trim()] = h.value;
      }
    }

    const start = performance.now();
    try {
      const res = await invoke<{
        status: number;
        status_text: string;
        body: string;
      }>("http_fetch", {
        request: {
          url: fullUrl,
          method: upperMethod,
          headers: headersObj,
          body: hasBody ? body : null,
          timeout_secs: 30,
        },
      });
      const elapsed = performance.now() - start;

      let isJson = false;
      try {
        JSON.parse(res.body);
        isJson = true;
      } catch {
        // not JSON
      }

      setResponse({
        status: res.status,
        statusText: res.status_text,
        body: res.body,
        timeMs: Math.round(elapsed),
        isJson,
      });
    } catch (err) {
      const elapsed = performance.now() - start;
      setResponse({
        status: 0,
        statusText: "Network Error",
        body: err instanceof Error ? err.message : String(err),
        timeMs: Math.round(elapsed),
        isJson: false,
      });
    } finally {
      setLoading(false);
    }
  };

  const statusColor = (status: number) => {
    if (status >= 200 && status < 300) return "bg-emerald-700 text-emerald-100";
    if (status >= 300 && status < 400) return "bg-amber-700 text-amber-100";
    return "bg-red-700 text-red-100";
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Badge
              variant="secondary"
              className={cn(
                HTTP_METHOD_COLOR[method as keyof typeof HTTP_METHOD_COLOR],
                "text-xs",
              )}
            >
              {upperMethod}
            </Badge>
            Invoke API
          </DialogTitle>
        </DialogHeader>

        {/* URL Bar */}
        <div className="flex items-center gap-2 p-2 rounded-lg border bg-muted/50">
          <code className="text-sm font-mono break-all flex-1">{fullUrl}</code>
        </div>

        {/* Path Parameters */}
        {pathParams.length > 0 && (
          <div className="space-y-2">
            <h4 className="text-sm font-medium">Path Parameters</h4>
            <div className="grid gap-2">
              {pathParams.map(param => (
                <div key={param} className="flex items-center gap-2">
                  <code className="text-xs font-mono w-32 shrink-0 text-muted-foreground">
                    {"{" + param + "}"}
                  </code>
                  <Input
                    placeholder={`Enter ${param}`}
                    value={paramValues[param] || ""}
                    onChange={e =>
                      setParamValues(prev => ({
                        ...prev,
                        [param]: e.target.value,
                      }))
                    }
                    className="h-8 text-sm"
                  />
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Headers & Body Tabs */}
        <Tabs defaultValue="headers">
          <TabsList>
            <TabsTrigger value="headers">
              Headers ({headers.length})
            </TabsTrigger>
            {hasBody && <TabsTrigger value="body">Body</TabsTrigger>}
          </TabsList>

          <TabsContent value="headers" className="space-y-2">
            {headers.map((header, i) => (
              <div key={i} className="flex items-center gap-2">
                <Input
                  placeholder="Header name"
                  value={header.key}
                  onChange={e => updateHeader(i, "key", e.target.value)}
                  className="h-8 text-sm flex-1"
                />
                <Input
                  placeholder="Value"
                  value={header.value}
                  onChange={e => updateHeader(i, "value", e.target.value)}
                  className="h-8 text-sm flex-1"
                />
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 shrink-0"
                  onClick={() => removeHeader(i)}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              </div>
            ))}
            <Button
              variant="outline"
              size="sm"
              onClick={addHeader}
              className="gap-1"
            >
              <Plus className="h-3 w-3" />
              Add Header
            </Button>
          </TabsContent>

          {hasBody && (
            <TabsContent value="body">
              <Textarea
                value={body}
                onChange={e => setBody(e.target.value)}
                placeholder='{"key": "value"}'
                className="font-mono text-sm min-h-[120px]"
              />
            </TabsContent>
          )}
        </Tabs>

        {/* Send Button */}
        <Button onClick={sendRequest} disabled={loading} className="gap-2">
          {loading ? (
            <>
              <Loader2 className="h-4 w-4 animate-spin" />
              Sending...
            </>
          ) : (
            "Send Request"
          )}
        </Button>

        {/* Response */}
        {response && (
          <div className="space-y-3 border-t pt-3">
            <div className="flex items-center gap-3">
              <Badge
                variant="secondary"
                className={statusColor(response.status)}
              >
                {response.status === 0
                  ? "Error"
                  : `${response.status} ${response.statusText}`}
              </Badge>
              <span className="text-xs text-muted-foreground">
                {response.timeMs}ms
              </span>
            </div>
            <div className="rounded-lg border p-3 overflow-auto max-h-[300px]">
              {response.isJson ? (
                <ReactJson
                  src={JSON.parse(response.body)}
                  name={null}
                  theme={resolvedTheme === "dark" ? "brewer" : lightJsonTheme}
                  collapsed={false}
                  enableClipboard={false}
                  displayDataTypes={false}
                  style={{ fontSize: "13px", lineHeight: "1.5" }}
                />
              ) : (
                <pre className="text-sm font-mono whitespace-pre-wrap break-all">
                  {response.body}
                </pre>
              )}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

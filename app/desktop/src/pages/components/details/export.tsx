import { ClipboardCopy, Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { useParams } from "react-router-dom";
import { ComponentList, Export, Parameter, Typ } from "@/types/component"; // ---------- Shadcn UI Tooltip Imports ----------
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { parseTypeForTooltip } from "@/lib/utils.ts";

/**
 * The interface for each export/function row
 * Now `parameter` and `return` can be React nodes.
 */
export interface ExportResult {
  package: string;
  function_name: string;
  parameter: React.ReactNode;
  return: React.ReactNode;
}

/**
 * A small component that renders the short name in a TooltipTrigger,
 * and shows the full multiline text in TooltipContent.
 */
function TypeWithPopover({ typ }: { typ: Typ | undefined }) {
  const { short, full } = parseTypeForTooltip(typ);
  const [copied, setCopied] = useState(false);
  const [open, setOpen] = useState(false);
  // const debouncedOpen = useDebounce(open, 200);

  const handleCopy = () => {
    navigator.clipboard.writeText(full).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000); // Reset after 2 seconds
    });
  };

  return (
    <Tooltip open={open} onOpenChange={setOpen}>
      <TooltipTrigger asChild onClick={() => setOpen(true)}>
        <span className="cursor-help text-emerald-400">{short}</span>
      </TooltipTrigger>
      <TooltipContent
        className="w-[350px] font-mono text-[13px] bg-zinc-900 border-zinc-700 text-zinc-100 p-0"
        side="right"
        sideOffset={5}
      >
        <div className="flex items-center justify-between bg-zinc-800 px-4 py-2 border-b border-zinc-700 space-y-1">
          <span className="font-semibold">Type Details</span>
          <button
            onClick={handleCopy}
            className="flex items-center text-xs text-blue-500 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300"
          >
            <ClipboardCopy className="w-4 h-4 mr-1" />
            {copied ? "Copied!" : "Copy"}
          </button>
        </div>
        <pre className="whitespace-pre-wrap text-sm bg-white dark:bg-zinc-900 p-2 rounded-md border dark:border-zinc-700 text-zinc-900 dark:text-zinc-100">
          {full}
        </pre>
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Builds a list of React nodes representing the parameters in
 * "paramName: <TypeWithTooltip>" format, separated by commas or line breaks.
 */
function buildParameterNodes(params: Parameter[]): React.ReactNode {
  return params.map((param, index) => {
    return (
      <span key={param.name}>
        <span className="text-blue-300">{param.name}</span>
        <span className="text-zinc-400">: </span>
        {/* <span className="text-yellow-600">{param.name}</span>
            {": "} */}
        <TypeWithPopover typ={param.typ} />
        {index < params.length - 1 && ", "}
      </span>
    );
  });
}

/**
 * Creates a list of ExportResult objects, where `parameter` and `return`
 * are now React nodes (with Shadcn Tooltips).
 */
function generateFunctionInterfacesV1(data: Export[]): ExportResult[] {
  const interfaces: ExportResult[] = [];

  data.forEach(exp => {
    exp.functions.forEach(func => {
      // Convert kebab-case to camelCase
      const functionName = func.name.replace(/-([a-z])/g, (_, letter: string) =>
        letter.toUpperCase(),
      );
      const paramNodes = buildParameterNodes(func.parameters);

      const returnNode = func.results?.[0]?.typ ? (
        <TypeWithPopover typ={func.results[0].typ} />
      ) : (
        <>void</>
      );

      interfaces.push({
        package: exp.name,
        function_name: functionName,
        parameter: paramNodes,
        return: returnNode,
      });
    });
  });

  return interfaces;
}

export default function Exports() {
  const { componentId = "" } = useParams();
  const [component, setComponent] = useState<ComponentList>({});
  const [versionList, setVersionList] = useState<number[]>([]);
  const [versionChange, setVersionChange] = useState<number>(0);
  const [result, setResult] = useState<ExportResult[]>([]);
  const [functions, setFunctions] = useState<ExportResult[]>([]);

  useEffect(() => {
    if (!componentId) return;

    // Fetch entire list of components by ID
    API.getComponentByIdAsKey().then(response => {
      const fetched = response[componentId];
      if (!fetched) return;

      const versions = fetched.versionList || [];
      setVersionList(versions);

      // Default to the latest version
      const selectedVersion = versions[versions.length - 1] || 0;
      setVersionChange(selectedVersion);

      setComponent(fetched);
    });
  }, [componentId]);

  useEffect(() => {
    if (!component.versions?.length) return;
    const componentDetails = component.versions.find(
      data => data.versionedComponentId?.version === versionChange,
    );
    if (!componentDetails) {
      setResult([]);
      setFunctions([]);
      return;
    }

    // Convert exports to the final interface format,
    // using our new "tooltip" parse logic
    const exportsResult: ExportResult[] = generateFunctionInterfacesV1(
      componentDetails.metadata?.exports || [],
    );
    setResult(exportsResult);
    setFunctions(exportsResult);

    // If you want to maintain a separate array of raw functions for searching:
    // const rawFunctions: ComponentExportFunction[] = calculateExportFunctions(
    //     componentDetails.metadata?.exports || []
    // );
    // setFunctions(rawFunctions);
  }, [component, versionChange]);

  const handleVersionChange = (version: number) => {
    setVersionChange(version);
  };

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toLowerCase();

    const searchResult = functions.filter((fn: ExportResult) =>
      fn.function_name.toLowerCase().includes(value),
    );
    setResult(searchResult);
  };

  return (
    <TooltipProvider>
      {/* The TooltipProvider ensures all nested Tooltips function correctly */}
      <div className="flex">
        <div className="flex-1 p-8">
          <div className="p-6 max-w-7xl mx-auto space-y-6">
            {/* Header */}
            <div className="flex justify-between items-center">
              <h1 className="text-2xl font-bold">Exports</h1>
            </div>

            {/* Search + Version Select */}
            <div className="flex items-center justify-between gap-10">
              <div className="relative flex-1 max-full">
                <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  placeholder="Search functions..."
                  className="pl-9"
                  onChange={e => handleSearch(e)}
                />
              </div>
              {versionList.length > 0 && (
                <Select
                  defaultValue={versionChange.toString()}
                  onValueChange={version => handleVersionChange(+version)}
                >
                  <SelectTrigger className="w-[80px]">
                    <SelectValue>v{versionChange}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {versionList.map((version: number) => (
                      <SelectItem key={version} value={String(version)}>
                        v{version}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </div>

            {/* Table of Exported Functions */}
            <div className="border rounded-lg">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[250px]">Function</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {result?.length > 0 ? (
                    result.map((fn: ExportResult) => (
                      <TableRow
                        key={`${fn.package}-${fn.function_name}`}
                        /* Combined key to reduce chance of collision */
                      >
                        <TableCell className="font-mono text-sm">
                          {/* Example: functionName(paramName: type, ...) => returnType */}
                          <span className="text-blue-400">{`${fn.package}.{${fn.function_name}}`}</span>{" "}
                          <span className="text-zinc-500">{"("}</span>
                          {fn.parameter}
                          <span className="text-zinc-500">{")"}</span> {"=>"}{" "}
                          {fn.return}
                        </TableCell>
                      </TableRow>
                    ))
                  ) : (
                    <TableRow>
                      <TableCell colSpan={2} className="text-center">
                        No exports found.
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </div>
          </div>
        </div>
      </div>
    </TooltipProvider>
  );
}

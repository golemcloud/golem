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
import { Case, ComponentList, Export, Parameter, Typ } from "@/types/component";

// ---------- Shadcn UI Tooltip Imports ----------
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";

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
 * Returns the short name and full multiline representation of a WIT-like type.
 */
function parseTypeForTooltip(typ: Typ | undefined): {
  short: string;
  full: string;
} {
  if (!typ) {
    return { short: "null", full: "null" };
  }

  switch (typ.type) {
    case "Bool":
      return { short: "bool", full: "bool" };
    case "S8":
    case "S16":
    case "S32":
    case "S64":
      return { short: `i${typ.type.slice(1)}`, full: `i${typ.type.slice(1)}` };
    case "U8":
    case "U16":
    case "U32":
    case "U64":
      return { short: `u${typ.type.slice(1)}`, full: `u${typ.type.slice(1)}` };
    case "F32":
    case "F64":
      return { short: typ.type.toLowerCase(), full: typ.type.toLowerCase() };
    case "Char":
      return { short: "char", full: "char" };
    case "Str":
      return { short: "string", full: "String" };
    case "List": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `list<${inner.short}>`,
        full: `Vec<${inner.full}>`,
      };
    }
    case "Option": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `option<${inner.short}>`,
        full: `Option<${inner.full}>`,
      };
    }
    case "Result": {
      const okParsed = parseTypeForTooltip(typ.ok);
      const errParsed = parseTypeForTooltip(typ.err);
      return {
        short: `result<${okParsed.short}, ${errParsed.short}>`,
        full: `Result<${okParsed.full}, ${errParsed.full}>`,
      };
    }
    case "Tuple": {
      const elements = (typ.fields || []).map((element) =>
        parseTypeForTooltip(element.typ)
      );
      return {
        short: `tuple<${elements.map((e) => e.short).join(", ")}>`,
        full: `(${elements.map((e) => e.full).join(", ")})`,
      };
    }
    case "Record": {
      const result: Record<string, unknown> = {};
      (typ.fields || []).forEach((field) => {
        const parsed = parseTypeForTooltip(field.typ);
        result[field.name] = parsed.full;
      });
      return {
        short: "record",
        full: JSON.stringify(result, null, 2),
      };
    }
    case "Variant": {
      const cases = ((typ.cases as Case[]) || []).map((c) => {
        const parsed = parseTypeForTooltip(c.typ);
        return `${c.name.charAt(0).toUpperCase() + c.name.slice(1)}(${
          parsed.full
        })`;
      });
      return {
        short: "variant",
        full: `enum {\n  ${cases.join(",\n  ")}\n}`,
      };
    }
    case "Enum": {
      const cases = ((typ.cases as string[]) || []).map(
        (c) => c.charAt(0).toUpperCase() + c.slice(1)
      );
      return {
        short: "enum",
        full: `enum (\n  ${cases.join(",\n  ")}\n)`,
      };
    }
    default:
      return { short: "unknown", full: "unknown" };
  }
}

/**
 * A small component that renders the short name in a TooltipTrigger,
 * and shows the full multiline text in TooltipContent.
 */
function TypeWithPopover({ typ }: { typ: Typ | undefined }) {
  const { short, full } = parseTypeForTooltip(typ);
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText(full).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000); // Reset after 2 seconds
    });
  };

  return (
    <Popover>
      <PopoverTrigger asChild>
        <span className="cursor-help text-blue-600 dark:text-blue-400">
          {short}
        </span>
      </PopoverTrigger>
      <PopoverContent className="p-4 bg-gray-100 dark:bg-gray-800 rounded-md shadow-lg">
        <div className="flex justify-between items-center mb-2">
          <span className="text-sm font-bold text-gray-900 dark:text-gray-100">
            Type Details
          </span>
          <button
            onClick={handleCopy}
            className="flex items-center text-xs text-blue-500 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300"
          >
            <ClipboardCopy className="w-4 h-4 mr-1" />
            {copied ? "Copied!" : "Copy"}
          </button>
        </div>
        <pre className="whitespace-pre-wrap text-sm bg-white dark:bg-gray-900 p-2 rounded-md border dark:border-gray-700 text-gray-900 dark:text-gray-100">
          {full}
        </pre>
      </PopoverContent>
    </Popover>
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
        <span className="text-yellow-600">{param.name}</span>
        {": "}
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

  data.forEach((exp) => {
    exp.functions.forEach((func) => {
      // Convert kebab-case to camelCase
      const functionName = func.name.replace(/-([a-z])/g, (_, letter: string) =>
        letter.toUpperCase()
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
  // const [functions, setFunctions] = useState<ComponentExportFunction[]>([]);

  useEffect(() => {
    if (!componentId) return;

    // Fetch entire list of components by ID
    API.getComponentByIdAsKey().then((response) => {
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
      (data) => data.versionedComponentId?.version === versionChange
    );
    if (!componentDetails) {
      setResult([]);
      setFunctions([]);
      return;
    }

    // Convert exports to the final interface format,
    // using our new "tooltip" parse logic
    const exportsResult: ExportResult[] = generateFunctionInterfacesV1(
      componentDetails.metadata?.exports || []
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
      fn.function_name.toLowerCase().includes(value)
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
                  onChange={(e) => handleSearch(e)}
                />
              </div>
              {versionList.length > 0 && (
                <Select
                  defaultValue={versionChange.toString()}
                  onValueChange={(version) => handleVersionChange(+version)}
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
                    <TableHead className="w-[250px]">Package</TableHead>
                    <TableHead className="w-[200px]">Function</TableHead>
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
                          {fn.package}
                        </TableCell>
                        <TableCell className="font-mono text-sm">
                          {/* Example: functionName(paramName: type, ...) => returnType */}
                          <span>{fn.function_name}</span>({fn.parameter}) {"=>"}{" "}
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

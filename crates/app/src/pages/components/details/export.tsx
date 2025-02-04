import { Search } from "lucide-react";
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
import {
  ComponentExportFunction,
  ComponentList,
  Export,
  Parameter,
  Typ,
} from "@/types/component";
import { calculateExportFunctions } from "@/lib/utils";

// ---------- Shadcn UI Tooltip Imports ----------
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

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
    case "Str":
      return { short: "string", full: "string" };
    case "U64":
      return { short: "number", full: "number" };
    case "Bool":
      return { short: "boolean", full: "boolean" };

    case "Option": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `option<${inner.short}>`,
        full: `option<${inner.full}>`,
      };
    }

    case "List": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `list<${inner.short}>`,
        full: `list<${inner.full}>`,
      };
    }

    case "Record": {
      // "short" is just 'record'; "full" is multiline representation
      const fields = (typ.fields || []).map((field) => {
        const parsed = parseTypeForTooltip(field.typ);
        return `  ${field.name}: ${parsed.full}`;
      });
      return {
        short: "record",
        full: `record {\n${fields.join("\n")}\n}`,
      };
    }

    case "Enum": {
      const cases = typ.cases?.map((c) => `'${c}'`).join(" | ") || "";
      return {
        short: "enum",
        full: `enum {\n  ${cases}\n}`,
      };
    }

    case "Result": {
      const okParsed = parseTypeForTooltip(typ.ok);
      const errParsed = parseTypeForTooltip(typ.err);
      return {
        short: `result<${okParsed.short}, ${errParsed.short}>`,
        full: `result<\n  ${okParsed.full},\n  ${errParsed.full}\n>`,
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
function TypeWithTooltip({ typ }: { typ: Typ | undefined }) {
  const { short, full } = parseTypeForTooltip(typ);
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="cursor-help text-blue-600">{short}</span>
      </TooltipTrigger>
      <TooltipContent>
        <pre className="whitespace-pre-wrap text-sm">{full}</pre>
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
        <span className="text-yellow-600">{param.name}</span>
        {": "}
        <TypeWithTooltip typ={param.typ} />
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
        <TypeWithTooltip typ={func.results[0].typ} />
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

  const [functions, setFunctions] = useState<ComponentExportFunction[]>([]);

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
    if (!component.versions?.length || versionChange === 0) return;

    const componentDetails = component.versions.find(
      (data) => data.versionedComponentId?.version === versionChange
    );
    if (!componentDetails) {
      setResult([]);
      return;
    }

    // Convert exports to the final interface format,
    // using our new "tooltip" parse logic
    const exportsResult: ExportResult[] = generateFunctionInterfacesV1(
      componentDetails.metadata?.exports || []
    );
    setResult(exportsResult);

    // If you want to maintain a separate array of raw functions for searching:
    const rawFunctions: ComponentExportFunction[] = calculateExportFunctions(
      componentDetails.metadata?.exports || []
    );
    setFunctions(rawFunctions);
  }, [component, versionChange]);

  const handleVersionChange = (version: number) => {
    setVersionChange(version);
  };

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toLowerCase();

    const searchResult = calculateExportFunctions(
      component.versions?.find(
        (data) => data.versionedComponentId?.version === versionChange
      )?.metadata?.exports || []
    ).filter((fn: ComponentExportFunction) =>
      fn.name.toLowerCase().includes(value)
    );

    setFunctions(searchResult);
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
                    <div className="p-4 align-center grid">
                      No exports found.
                    </div>
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

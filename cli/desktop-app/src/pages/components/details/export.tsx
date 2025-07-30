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
import { ComponentList } from "@/types/component";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

function FunctionDisplay({ funcStr }: { funcStr: string }) {
  const [_copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText(funcStr).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };

  // Simple regex to extract parts for coloring
  const functionMatch = funcStr.match(/\.{([^}]+)}/);
  const paramsMatch = funcStr.match(/\(([^)]*)\)/);
  const returnMatch = funcStr.match(/-> (.+)$/);

  const functionName = functionMatch ? functionMatch[1] : "";
  const params = paramsMatch ? paramsMatch[1] : "";
  const returnType = returnMatch ? returnMatch[1] : "";

  // Find the positions to split the string for coloring
  const beforeFunction = funcStr.substring(0, funcStr.indexOf(".{") + 2);

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="cursor-help font-mono text-sm">
          <span className="text-zinc-400">{beforeFunction}</span>
          <span className="text-purple-400">{functionName}</span>
          <span className="text-zinc-400"> {"}"} (</span>
          <span className="text-blue-300">{params}</span>
          <span className="text-zinc-400">)</span>
          {returnType && (
            <>
              <span className="text-zinc-400"> → </span>
              <span className="text-emerald-400">{returnType}</span>
            </>
          )}
        </span>
      </TooltipTrigger>
      <TooltipContent className="flex items-center gap-2">
        <span className="font-mono text-xs">{funcStr}</span>
        <button
          onClick={handleCopy}
          className="text-xs text-blue-400 hover:text-blue-300"
        >
          <ClipboardCopy className="w-3 h-3" />
        </button>
      </TooltipContent>
    </Tooltip>
  );
}

export default function Exports() {
  const { componentId = "", appId } = useParams();
  const [component, setComponent] = useState<ComponentList>({});
  const [versionList, setVersionList] = useState<number[]>([]);
  const [versionChange, setVersionChange] = useState<number>(0);
  const [result, setResult] = useState<string[]>([]);
  const [functions, setFunctions] = useState<string[]>([]);

  useEffect(() => {
    if (!componentId) return;
    API.componentService.getComponentByIdAsKey(appId!).then(response => {
      const fetched = response[componentId];
      if (!fetched) return;

      const versions = fetched.versionList || [];
      setVersionList(versions);

      const selectedVersion = versions[versions.length - 1] || 0;
      setVersionChange(selectedVersion);
      setComponent(fetched);
    });
  }, [componentId]);

  useEffect(() => {
    if (!component.versions?.length) return;
    const componentDetails = component.versions.find(
      data => data.componentVersion === versionChange,
    );
    if (!componentDetails) {
      setResult([]);
      setFunctions([]);
      return;
    }

    const exports = componentDetails.exports || [];
    setResult(exports);
    setFunctions(exports);
  }, [component, versionChange]);

  const handleVersionChange = (version: number) => {
    setVersionChange(version);
  };

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toLowerCase();
    const searchResult = functions.filter((funcStr: string) =>
      funcStr.toLowerCase().includes(value),
    );
    setResult(searchResult);
  };

  return (
    <TooltipProvider>
      <div className="flex">
        <div className="flex-1 p-8">
          <div className="p-6 max-w-7xl mx-auto space-y-6">
            <div className="flex justify-between items-center">
              <h1 className="text-2xl font-bold">Exports</h1>
            </div>

            <div className="flex items-center justify-between gap-10">
              <div className="relative flex-1 max-full">
                <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  placeholder="Search functions..."
                  className="pl-9"
                  onChange={handleSearch}
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

            <div className="border rounded-lg">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-full">Function Signature</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {result?.length > 0 ? (
                    result.map((funcStr: string, index) => (
                      <TableRow key={index}>
                        <TableCell>
                          <FunctionDisplay funcStr={funcStr} />
                        </TableCell>
                      </TableRow>
                    ))
                  ) : (
                    <TableRow>
                      <TableCell className="text-center">
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

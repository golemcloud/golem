import { useParams } from "react-router-dom";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
// import { Download } from "lucide-react";
import { useEffect, useState } from "react";
import { ComponentList } from "@/types/component";
// import { saveFile } from "@/lib/tauri&web.ts";
import { API } from "@/service";
import { formatRelativeTime } from "@/lib/utils";
// import { toast } from "@/hooks/use-toast";

export default function ComponentInfo() {
  const { componentId = "", appId } = useParams();
  const [componentList, setComponentList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const [versionList, setVersionList] = useState([] as number[]);
  const [versionChange, setVersionChange] = useState(0 as number);

  useEffect(() => {
    if (componentId) {
      API.componentService.getComponentByIdAsKey(appId!).then(response => {
        const componentData = response[componentId];
        const versionList = componentData?.versionList || [];
        setVersionList(versionList);
        setComponentList(response);
        if (versionList.length > 0) {
          setVersionChange(versionList[versionList.length - 1]!);
        }
      });
    }
  }, [componentId]);

  const handleVersionChange = (version: number) => {
    setVersionChange(version);
  };

  // async function downloadFile() {
  //   try {
  //     API.downloadComponent(componentId!, versionChange).then(
  //       async response => {
  //         const blob = await response.blob();
  //         const arrayBuffer = await blob.arrayBuffer();

  //         const fileName = `${componentId}.wasm`;
  //         await saveFile(fileName, new Uint8Array(arrayBuffer));
  //         toast({
  //           title: "File downloaded successfully",
  //           duration: 3000,
  //         });
  //       },
  //     );
  //   } catch (error) {
  //     console.error("Error downloading the file:", error);
  //   }
  // }

  const componentDetails =
    componentList[componentId]?.versions?.[versionChange] || {};

  return (
    <div className="flex justify-center p-8">
      <Card className="w-full max-w-4xl shadow-md border rounded-xl">
        <CardHeader className="flex flex-col md:flex-row items-center justify-between pb-6 border-b">
          <div className="text-center md:text-left">
            <CardTitle className="text-2xl font-semibold">
              Component Information
            </CardTitle>
            <CardDescription>
              View metadata about this component
            </CardDescription>
          </div>
          <div className="flex items-center gap-3 mt-4 md:mt-0">
            {versionList.length > 0 && (
              <Select
                defaultValue={versionChange.toString()}
                onValueChange={version => handleVersionChange(+version)}
              >
                <SelectTrigger className="w-[100px] border rounded-md">
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
            {/* <Button
              variant="outline"
              size="icon"
              className="border rounded-md"
              onClick={downloadFile}
            >
              <Download className="h-5 w-5" />
            </Button> */}
          </div>
        </CardHeader>
        <CardContent>
          <div className="space-y-4 text-sm">
            {[
              ["Component ID", componentDetails.componentId],
              ["Version", versionChange],
              ["Name", componentDetails.componentName],
              [
                "Size",
                `${Math.round((componentDetails?.componentSize || 0) / 1024)} KB`,
              ],
              [
                "Created At",
                componentDetails.createdAt
                  ? formatRelativeTime(componentDetails.createdAt)
                  : "NA",
              ],
            ].map(([label, value], index) => (
              <div
                key={label}
                className={`grid grid-cols-[180px,1fr] items-center gap-4 py-3 ${
                  index !== 4 ? "border-b" : ""
                }`}
              >
                <div className="font-medium text-muted-foreground">{label}</div>
                <div className="font-mono">{value}</div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

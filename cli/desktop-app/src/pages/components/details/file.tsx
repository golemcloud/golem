import { FolderStructure } from "@/components/file-manager.tsx";
import { useParams } from "react-router-dom";
import { useEffect, useState } from "react";
import { API } from "@/service";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ComponentList } from "@/types/component.ts";

export default function FileManager() {
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

  const componentDetails =
    componentList[componentId]?.versions?.[versionChange] || {};

  return (
    <div className="container mx-auto p-4 gap-4 flex flex-col">
      <div className="flex justify-between">
        <div className="relative flex-1 max-full">
          <h1 className="text-2xl font-bold">Files Systems</h1>
        </div>
        <div>
          {versionList.length > 0 && (
            <Select
              defaultValue={versionChange.toString()}
              onValueChange={version => handleVersionChange(+version)}
            >
              <SelectTrigger className="w-[80px]">
                <SelectValue> v{versionChange}</SelectValue>
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
      </div>

      <FolderStructure data={componentDetails.files || []} />
    </div>
  );
}

import { useParams, useRouter, useSearchParams,usePathname } from "next/navigation";
import { MultiSelect } from "@/components/ui/multi-select";
import React, {useEffect, useMemo, useRef, useState, useCallback } from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";


export function VersionFilter() {
    const router = useRouter();
    const { apiId } = useParams<{ apiId: string }>();
    const pathname = usePathname();
    const { apiDefinitions, getApiDefintion, isLoading } =
    useApiDefinitions(apiId);
    
    const params = useSearchParams();

    const versions = useMemo(() => {
      return apiDefinitions.map((api) => {
        return {label: api.version, value:api.version};
      });
    }, [apiDefinitions]);
  
    // Using useRef to keep track of the selected version
    const selectedVersionRef = useRef<string[]>([versions[0]?.value]);
  
    // Sync selected version from search params
    useEffect(() => {
      const version = params?.get("workerVersion");
      if (version) {
        try {
          const parsedVersion = JSON.parse(version)?.version;
          if (parsedVersion !== undefined) {
            selectedVersionRef.current = [`${parsedVersion}`];
          } else {
            selectedVersionRef.current = ["-1"];
          }
        } catch (err) {
          console.error("Error parsing workerVersion:", err);
          selectedVersionRef.current = ["-1"];
        }
      } else {
        selectedVersionRef.current = ["-1"];
      }
    }, [params]);

    const tab = useMemo(() => {
      const parts = pathname?.split("/") || [];
      return parts[parts.length - 1] || "overview";
    }, [pathname]);
  
    const handleChange = (value: string[]) => {
      console.log(value);
      router.push(`/apis/${apiId}/${tab}?version=${value[0]}`);
    };
  
  
    return (
      <div className="max-w-5">
        <MultiSelect
          selectMode="single"
          buttonType={{variant:"success", size:"icon_sm"}}
          options={versions}
          onValueChange={handleChange}
          defaultValue={selectedVersionRef.current}
          className="min-w-15"
          variant="inverted"
          animation={2}
          maxCount={2}
        />
      </div>
    );
  }
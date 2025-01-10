import { useParams, useRouter,usePathname, useSearchParams } from "next/navigation";
import { MultiSelect } from "@/components/ui/multi-select";
import React, { useMemo } from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";


export function VersionFilter() {
    const router = useRouter();
    const { apiId } = useParams<{ apiId: string }>();
    const params = useSearchParams();
    const pathname = usePathname();

    const { apiDefinitions, getApiDefintion, isLoading } = useApiDefinitions(apiId);

    const {data: apiDefinition} = (!isLoading && getApiDefintion(apiId, params.get("version"))) || {}
    

    const versions = useMemo(() => {
      return apiDefinitions.map((api) => {
        return {label: api.version, value:api.version};
      });
    }, [apiDefinitions]);
  
    const tab = useMemo(() => {
      const parts = pathname?.split("/") || [];
      return parts[parts.length - 1] || "overview";
    }, [pathname]);
  

    const handleChange = (value:string[]) => {
      if(!value){
        return;
      }
      router.push(`/apis/${apiId}/${tab}?version=${value[0]}`);
    };
  
    return (
      <div className="max-w-5">
        {apiDefinition && <MultiSelect
          selectMode="single"
          buttonType={{variant:"success", size:"icon_sm"}}
          options={versions}
          onValueChange={handleChange}
          defaultValue={[apiDefinition?.version]}
          className="min-w-15"
          variant="inverted"
          animation={2}
          maxCount={2}
        />}
      </div>
    );
  }
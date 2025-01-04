import { useParams, useRouter,usePathname } from "next/navigation";
import { MultiSelect } from "@/components/ui/multi-select";
import React, {useMemo} from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";


export function VersionFilter() {
    const router = useRouter();
    const { apiId } = useParams<{ apiId: string }>();
    const pathname = usePathname();
    const { apiDefinitions } =
    useApiDefinitions(apiId);
    

    const versions = useMemo(() => {
      return apiDefinitions.map((api) => {
        return {label: api.version, value:api.version};
      });
    }, [apiDefinitions]);

    const tab = useMemo(() => {
      const parts = pathname?.split("/") || [];
      return parts[parts.length - 1] || "overview";
    }, [pathname]);
  
    const handleChange = (value: string[]) => {
      router.push(`/apis/${apiId}/${tab}?version=${value[0]}`);
    };
  
    return (
      <div className="max-w-5">
        <MultiSelect
          selectMode="single"
          buttonType={{variant:"success", size:"icon_sm"}}
          options={versions}
          onValueChange={handleChange}
          defaultValue={[versions[0]?.value]}
          className="min-w-15"
          variant="inverted"
          animation={2}
          maxCount={2}
        />
      </div>
    );
  }
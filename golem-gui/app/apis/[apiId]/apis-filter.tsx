import { useRouter, usePathname, useSearchParams } from "next/navigation";
import { MultiSelect } from "@/components/ui/multi-select";
import React, { useMemo } from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { Box, Typography } from "@mui/material";
import DraftsIcon from "@mui/icons-material/Drafts";
import DoneIcon from "@mui/icons-material/Done";



export function VersionFilter({ showFilter }: { showFilter?: boolean }) {
  const router = useRouter();
  const { apiId } = useCustomParam();
  const params = useSearchParams();
  const pathname = usePathname();
  const { apiDefinitions, getApiDefintion, isLoading } =
    useApiDefinitions(apiId);

  // Extract version explicitly
  const version = params.get("version");

  const { data: apiDefinition } =  (!isLoading && getApiDefintion(apiId, version)) || { data: null };

  const versions = useMemo(() => {
    return apiDefinitions.map((api) => {
      return { label: api.version, value: api.version };
    });
  }, [apiDefinitions]);

  const tab = useMemo(() => {
    const parts = pathname?.split("/") || [];
    return parts[parts.length - 1] || "overview";
  }, [pathname]);

  const handleChange = 
    (value: string[]) => {
      if (!value) {
        return;
      }
     
      if (["overview", "settings", "playground", "deployments"].includes(tab)) {
        return router.push(`/apis/${apiId}/${tab}?version=${value[0]}`);
      } else {
        const {data: apiDefinition} = getApiDefintion(apiId, value[0])
        const route = apiDefinition ? apiDefinition?.routes[0] : null;
        const newtab =
          (route && encodeURIComponent(`${route?.path}|${route?.method}`)) ||
          "overview";

        return router.push(`/apis/${apiId}/${newtab}/?version=${value[0]}`);
      }
    }

  const apiName = (apiDefinition && apiDefinition?.id) || "";

  return (
    <Box className="flex gap-3">
      <div className="w-12">
        {showFilter && (
          <div className="max-w-5">
            {apiDefinition && (
              <MultiSelect
                selectMode="single"
                buttonType={{ variant: "success", size: "icon_sm" }}
                options={versions}
                onValueChange={handleChange}
                defaultValue={[apiDefinition?.version]}
                className="min-w-15"
                variant="inverted"
                animation={2}
                maxCount={2}
              />
            )}
          </div>
        )}
      </div>
      <Typography variant="body2" className="text-bold">
        {apiName.length > 15 ? `${apiName.slice(0, 15)}...` : apiName}
      </Typography>
      {apiDefinition &&
        (apiDefinition.draft ? (
          <DraftsIcon fontSize="small" className="text-yellow-500 self-end" />
        ) : (
          <DoneIcon className="text-green-600 self-end" fontSize="small" />
        ))}
    </Box>
  );
}

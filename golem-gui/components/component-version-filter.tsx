import { useRouter, usePathname, useSearchParams } from "next/navigation";
import { MultiSelect } from "@/components/ui/multi-select";
import React, { useMemo } from "react";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { Box, Typography } from "@mui/material";
import useComponents from "@/lib/hooks/use-component";
import { Component } from "@/types/api";

export function ComponentVersionFilter({
  showFilter,
}: {
  showFilter?: boolean;
}) {
  const router = useRouter();
  const { compId } = useCustomParam();
  const params = useSearchParams();
  const pathname = usePathname();
  const { components, getComponent, isLoading } = useComponents(compId);

  // Extract version explicitly
  const version = params.get("version");

  const { data: component } = (!isLoading && getComponent(compId, version)) || {
    data: null,
  };

  const versions = useMemo(() => {
    return components.map((component: Component) => {
      return {
        label: `V${component.versionedComponentId.version}`,
        value: `${component.versionedComponentId.version}`,
      };
    });
  }, [components]);

  const tab = useMemo(() => {
    const parts = pathname?.split("/") || [];
    return parts[parts.length - 1] || "overview";
  }, [pathname]);

  const handleChange = (value: string[]) => {
    if (!value) {
      return;
    }
    return router.push(`/components/${compId}/${tab}?version=${value[0]}`);
  };

  const componentName = (component && component?.componentName) || "";

  return (
    <Box className="flex gap-3">
      <div className="w-12">
        {showFilter && (
          <div className="max-w-5">
            {component && (
              <MultiSelect
                selectMode="single"
                buttonType={{ variant: "success", size: "icon_sm" }}
                options={versions}
                onValueChange={handleChange}
                defaultValue={[`${component?.versionedComponentId?.version}`]}
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
        {componentName.length > 15
          ? `${componentName.slice(0, 15)}...`
          : componentName}
      </Typography>
    </Box>
  );
}

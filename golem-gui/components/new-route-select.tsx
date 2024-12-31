import React from "react";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Controller } from "react-hook-form";

interface ComponentSelectProps {
  name: string;
  label: string;
  control: any;
  component?: string;
  options: any;
  isLoading: boolean;
}

export function ComponentSelect({
  name,
  label,
  control,
  component,
  options,
  isLoading,
}: ComponentSelectProps) {
  const seenIds = new Set();
  const uniqueOptions =
    name == "component"
      ? options.filter((option: any) => {
          const id = option.versionedComponentId?.componentId;
          if (seenIds.has(id)) {
            return false;
          }
          seenIds.add(id);
          return true;
        })
      : options;

  return (
    <div className="w-full">
      <Controller
        name={name}
        control={control}
        rules={{ required: `${label.split(" ")[1]} is mandatory!` }}
        render={({ field }) => (
          <Select
            {...field}
            value={field.value || ""}
            onValueChange={(value) => field.onChange(value)}
            disabled={isLoading || options.length === 0}
          >
            <SelectTrigger>
              <SelectValue placeholder={field.value || label} />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectLabel>{label}</SelectLabel>
                {isLoading ? (
                  <SelectItem value="" disabled>
                    Loading...
                  </SelectItem>
                ) : (
                  uniqueOptions.map((option: any) => {
                    if (name === "version") {
                      return option?.versionedComponentId?.componentId ==
                        component ? (
                        <SelectItem
                          key={`${option?.versionedComponentId?.componentId}__${option.versionedComponentId.version}`}
                          value={option.versionedComponentId.version}
                        >
                          {option.versionedComponentId.version}
                        </SelectItem>
                      ) : null;
                    } else if (name === "component") {
                      return (
                        <SelectItem
                          key={option?.versionedComponentId?.componentId}
                          value={option?.versionedComponentId?.componentId}
                        >
                          {option.componentName}
                        </SelectItem>
                      );
                    }
                  })
                )}
              </SelectGroup>
            </SelectContent>
          </Select>
        )}
      />
    </div>
  );
}

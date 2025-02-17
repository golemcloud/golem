import React from "react";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@ui/select";
import { Control, Controller} from "react-hook-form";

interface Option {
  id: string | number;
  name: string;
  componentName: string;
}

interface CustomSelectProps {
  name: string;
  label: string;
  control?:Control;
  options: Option[];
  isLoading: boolean;
}

export function CustomSelect({
  name,
  label,
  control,
  options,
  isLoading,
}: CustomSelectProps) {
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
                  options.map((option: Option) => {
                    return (
                      <SelectItem
                        key={option?.id}
                        value={option?.name}
                      >
                        {option.componentName}
                      </SelectItem>
                    );
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

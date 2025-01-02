"use client";

import React, {useEffect, useMemo, useRef, useState, useCallback } from "react";
import { MultiSelect } from "@/components/ui/multi-select";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { TextField, InputAdornment } from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import useComponents from "@/lib/hooks/use-component";
import { DatePicker } from "@/components/ui/date-picker";

const statuses = [
  { value: "Running", label: "Running" },
  { value: "Idle", label: "Idle" },
  { value: "Suspended", label: "Suspended" },
  { value: "Interrupted", label: "Interrupted" },
  { value: "Retrying", label: "Retrying" },
  { value: "Failed", label: "Failed" },
  { value: "Exited", label: "Exited" },
];


// TODO Filter logic can be made more generic.
export function StatusFilter() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { compId } = useParams<{ compId: string }>();

  // Using useRef to store selectedStatus
  const selectedStatusRef = useRef<string[]>(["Running"]);

  useEffect(() => {
    const workerStatus = searchParams?.get("workerStatus");

    if (workerStatus) {
      try {
        const parsedWorkStatus = JSON.parse(workerStatus);
        if (parsedWorkStatus !== undefined) {
          selectedStatusRef.current = parsedWorkStatus;
        } else {
          selectedStatusRef.current = [];
        }
      } catch (err) {
        console.error("Error parsing workerStatus:", err);
        selectedStatusRef.current = [];
      }
    } else {
      selectedStatusRef.current = [];
    }
  }, [searchParams]);

  const handleChange = (value: string[]) => {
    const params = new URLSearchParams(searchParams);
    if (value.length > 0) {
      params.set("workerStatus", JSON.stringify(value));
    } else {
      params.delete("workerStatus");
    }
    selectedStatusRef.current = value; // Update ref directly
    router.push(`/components/${compId}/workers?${params.toString()}`);
  };

  return (
    <div className="max-w-40">
      <MultiSelect
        options={statuses}
        onValueChange={handleChange}
        defaultValue={selectedStatusRef.current} // Use ref value
        placeholder="Status"
        variant="inverted"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}


export function VersionFilter() {
  const router = useRouter();
  const { compId } = useParams<{ compId: string }>();
  const searchParams = useSearchParams();
  const { components, isLoading } = useComponents(compId);

  const versions = useMemo(() => {
    if (isLoading) return [];
    return [
      { value: "-1", label: "Any" },
      ...(components?.map((component) => ({
        value: `${component.versionedComponentId.version}`,
        label: `V${component.versionedComponentId.version}`,
      })) || []),
    ];
  }, [isLoading, components]);

  // Using useRef to keep track of the selected version
  const selectedVersionRef = useRef<string[]>(["-1"]);

  // Sync selected version from search params
  useEffect(() => {
    const version = searchParams?.get("workerVersion");
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
  }, [searchParams]);

  const handleChange = (value: string[]) => {
    const params = new URLSearchParams(searchParams);
    const parsedValue = Number(value?.[0] || "-1");
    if (!isNaN(parsedValue) && parsedValue >= 0) {
      params.set(
        "workerVersion",
        JSON.stringify({ version: parsedValue, comparator: "Equal" })
      );
    } else {
      params.delete("workerVersion");
    }
    router.push(`/components/${compId}/workers?${params.toString()}`);
  };

  console.log("selectedVersionRef.current", selectedVersionRef.current);

  return (
    <div className="max-w-40">
      <MultiSelect
        selectMode="single"
        options={versions}
        onValueChange={handleChange}
        defaultValue={selectedVersionRef.current}
        placeholder="Version"
        variant="inverted"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}


interface SearchProps {
  placeholder?: string;
}

export const Search = ({ placeholder = "Worker Name..." }: SearchProps) => {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { compId } = useParams<{ compId: string }>();
  const [searchQuery, setSearchQuery] = useState("");
  const debounceTimeout = useRef<NodeJS.Timeout | null>(null);

  useEffect(() => {
    const workerName = searchParams?.get("workerName");
    if (workerName) {
      try {
        const name = JSON.parse(workerName)?.search;
        if (name) {
          setSearchQuery(name);
        } else {
          setSearchQuery("");
        }
      } catch (err) {
        console.error("Error parsing workerName:", err);
        setSearchQuery("");
      }
    } else {
      setSearchQuery("");
    }
  }, [searchParams]);

  // Debounced search handler
  const handleSearch = useCallback(
    (value: string) => {
      const params = new URLSearchParams(searchParams);
      if (value) {
        params.set(
          "workerName",
          JSON.stringify({
            search: value,
            comparator: "Like"
          })
        );
      } else {
        params.delete("workerName");
      }
      router.push(`/components/${compId}/workers?${params.toString()}`);
    },
    [searchParams, router, compId]
  );

  // Debounce input changes
  const handleInputChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = event.target.value;
    setSearchQuery(value);

    // Clear previous timeout
    if (debounceTimeout.current) {
      clearTimeout(debounceTimeout.current);
    }

    // Set a new debounce timeout
    debounceTimeout.current = setTimeout(() => {
      handleSearch(value);
    }, 300); // Adjust debounce delay as needed
  };


  return (
    <TextField
      placeholder={placeholder}
      variant="outlined"
      className="flex-1"
      defaultValue={searchQuery}
      size="small"
      onChange={handleInputChange}
      InputProps={{
        startAdornment: (
          <InputAdornment position="start">
            <SearchIcon sx={{ color: "grey.500" }} />
          </InputAdornment>
        ),
      }}
    />
  );
};



export function CustomDatePickFilter({label, searchKey}:{
  label:string, searchKey:string
}) {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { compId } = useParams<{ compId: string }>();

  // Using useRef to store selectedStatus
  const selectedDateRef = useRef<Date>();

  useEffect(() => {
    const workerDate = searchParams?.get(searchKey);
    if (workerDate) {
      try {
        const parsedWorkerDate = JSON.parse(workerDate);
        if (parsedWorkerDate !== undefined) {
          selectedDateRef.current = new Date(parsedWorkerDate.value);
        } else {
          selectedDateRef.current = undefined;

        }
      } catch (err) {
        console.error("Error parsing workerStatus:", err);
        selectedDateRef.current = undefined;
      }
    } else {
      selectedDateRef.current = undefined;
    }
  }, [searchParams]);

  const handleChange = (value?: Date) => {
    const params = new URLSearchParams(searchParams);
    if (value) {
      params.set(searchKey, JSON.stringify({
        "type": "absolute",
        "value": value.toISOString()
    }));
    } else {
      params.delete(searchKey);
    }
    selectedDateRef.current = value ? new Date(value) : value; // Update ref directly
    router.push(`/components/${compId}/workers?${params.toString()}`);
  };

  return (
      <DatePicker handleChange={handleChange} 
      defaultValue={selectedDateRef.current} 
      label={label}
      />
  );
}


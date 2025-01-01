"use client";

import React, { useState } from "react";
import { MultiSelect } from "@/components/ui/multi-select"; 

const statuses = [
  { value: "running", label: "Running" },
  { value: "idle", label: "Idle" },
  { value: "suspended", label: "Suspended" },
  { value: "interrupted", label: "Interrupted" },
  { value: "retrying", label: "Retrying" },
  { value: "failed", label: "Failed" },
  { value: "exited", label: "Exited" }
]

export function StatusFilter() {

  const [selectedStatus, setSelectedStatus] = useState(['running']);

  return (
    <div className="max-w-40 ">
      <MultiSelect
        options={statuses}
        onValueChange={setSelectedStatus}
        value={selectedStatus}
        placeholder="Status"
        variant="inverted"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}

export function VersionFilter() {

  const version = [
    { value: "Any", label: "Any" },
    { value: "v1", label: "v1" },
    { value: "v2", label: "v2" },
  ]

  const [selectedVersion, setSelectedVersion] = useState(['Any']);

  return (
    <div className="max-w-40 ">
      <MultiSelect
        options={version}
        onValueChange={setSelectedVersion}
        value={selectedVersion}
        placeholder="Version"
        variant="inverted"
        selectMode="single"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}
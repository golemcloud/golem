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


function DropDown() {

  const [selectedStatus, setSelectedStatus] = useState(['running']);

  return (
    <div className="max-w-40 ">
      <MultiSelect
        options={statuses}
        onValueChange={setSelectedStatus}
        value={selectedStatus}
        placeholder="Select"
        variant="inverted"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}

export default DropDown;
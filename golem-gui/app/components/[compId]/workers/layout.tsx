"use client";
import { useSearchParams } from "next/navigation";
import React from "react";
import useComponents from "@/lib/hooks/use-component";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function WorkersLayout({
  workers,
  workersephemeral,
}: {
    workers: React.ReactNode;
    workersephemeral: React.ReactNode;
}) {
  const { compId } = useCustomParam();
  const params = useSearchParams();
  const version = params?.get("version");
  const component = useComponents(compId!, version);
  const type = component?.components[0]?.componentType;
  if(!type) return <>Loading...</>;
  return type === 'Durable' ? workers : workersephemeral;
}

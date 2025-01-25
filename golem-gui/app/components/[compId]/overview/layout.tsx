"use client";

import useComponents from "@/lib/hooks/use-component";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { useSearchParams } from "next/navigation";
import React from "react";

export default function OverviewLayout({
  overviewdurable,
  overviewephemeral,
}: {
  overviewdurable?: React.ReactNode;
  overviewephemeral?: React.ReactNode;
}) {
  const { compId } = useCustomParam();
  const params = useSearchParams();
  const version = params?.get("version");
  const component = useComponents(compId!, version);
  const type = component?.components[0]?.componentType;
  if(!type) return <>Loading...</>;
  return type === 'Ephemeral' ? overviewephemeral : overviewdurable;
}

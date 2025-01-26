"use client";

import useComponents from "@/lib/hooks/use-component";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import React from "react";

export default function OverviewLayout({
  overviewdurable,
  overviewephemeral,
}: {
  overviewdurable?: React.ReactNode;
  overviewephemeral?: React.ReactNode;
}) {
  const { compId } = useCustomParam();
  const component = useComponents(compId, "latest");
  const type = component?.components[0]?.componentType;
  if(!type) return <>Loading...</>;
  return type === 'Ephemeral' ? overviewephemeral : overviewdurable;
}

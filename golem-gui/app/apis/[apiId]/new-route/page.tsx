"use client";
import React from "react";
import { useParams, useSearchParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function Page() {
  // const {apiId} =useParams<{apiId:string}>()
  const { apiId } = useCustomParam();
  const searchParams = useSearchParams(); 
  return <NewRouteForm apiId={apiId} version={searchParams?.get("version") || ""}/>
}
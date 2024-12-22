"use client";
import React from "react";
import { useParams, useSearchParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";

export default function Page() {
  const {apiId} =useParams<{apiId:string}>()
  const searchParams = useSearchParams(); 
  return <NewRouteForm apiId={apiId} version={searchParams?.get("version") || ""}/>
}
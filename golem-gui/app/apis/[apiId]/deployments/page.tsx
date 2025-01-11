"use client";
import DeploymentPage from "@/components/deployment";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function Page() {
  const { apiId } = useCustomParam();
  return <DeploymentPage apiId={apiId}/>
 }

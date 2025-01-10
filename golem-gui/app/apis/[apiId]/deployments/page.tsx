"use client";
import { useParams } from "next/navigation";
import DeploymentPage from "@/components/deployment";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function Page() {
  // const { apiId } = useParams<{ apiId: string }>();
  const { apiId } = useCustomParam();
  return <DeploymentPage apiId={apiId}/>
 }

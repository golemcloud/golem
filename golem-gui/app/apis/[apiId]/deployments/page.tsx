"use client";
import { useParams } from "next/navigation";
import DeploymentPage from "@/components/deployment";

export default function Page() {
  const { apiId } = useParams<{ apiId: string }>();
  return <DeploymentPage apiId={apiId}/>
 }

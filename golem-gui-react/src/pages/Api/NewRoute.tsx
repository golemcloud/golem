import { useSearchParams } from "react-router-dom";
import NewRouteForm from "@components/apis/new-route";
import { useCustomParam } from "@lib/hooks/use-custom-param";

export default function Page() {
  const { apiId } = useCustomParam();
  const [searchParams] = useSearchParams(); 
  return <NewRouteForm apiId={apiId} version={searchParams?.get("version") || ""}/>
}
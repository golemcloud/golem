import DeploymentPage from "@components/apis/deployment";
import { useCustomParam } from "@lib/hooks/use-custom-param";

export default function Page() {
  const { apiId } = useCustomParam();
  return <DeploymentPage apiId={apiId}/>
 }

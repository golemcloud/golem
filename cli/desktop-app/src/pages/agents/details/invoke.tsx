import { useInvoke } from "@/hooks/useInvoke";
import { InvokeLayout } from "@/components/invoke/InvokeLayout";

export default function AgentInvoke() {
  const {
    functionDetails,
    value,
    setValue,
    resultValue,
    setResultValue,
    viewMode,
    setViewMode,
    parsedExports,
    name,
    urlFn,
    appId,
    componentId,
    agentName,
    handleValueChange,
    onInvoke,
    copyToClipboard,
    navigate,
  } = useInvoke({ isAgentInvoke: true });

  const onNavigateToFunction = (exportName: string, functionName: string) => {
    navigate(
      `/app/${appId}/components/${componentId}/agents/${agentName}/invoke?name=${exportName}&fn=${functionName}`,
    );
  };

  return (
    <InvokeLayout
      parsedExports={parsedExports}
      name={name}
      urlFn={urlFn}
      onNavigateToFunction={onNavigateToFunction}
      functionDetails={functionDetails}
      viewMode={viewMode}
      setViewMode={setViewMode}
      value={value}
      setValue={setValue}
      resultValue={resultValue}
      setResultValue={setResultValue}
      onValueChange={handleValueChange}
      onInvoke={onInvoke}
      copyToClipboard={copyToClipboard}
    />
  );
}

import { useInvoke } from "@/hooks/useInvoke";
import { InvokeLayout } from "@/components/invoke/InvokeLayout";

export default function WorkerInvoke() {
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
    workerName,
    handleValueChange,
    onInvoke,
    copyToClipboard,
    navigate,
  } = useInvoke({ isWorkerInvoke: true });

  const onNavigateToFunction = (exportName: string, functionName: string) => {
    navigate(
      `/app/${appId}/components/${componentId}/workers/${workerName}/invoke?name=${exportName}&&fn=${functionName}`,
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

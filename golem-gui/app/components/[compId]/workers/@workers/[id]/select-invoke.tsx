import { MultiSelect } from "@/components/ui/multi-select";
import { useMemo } from "react";


type WorkerFunction = {
    name: string;
    parameters?: any[];
    results?: any[];
    type?: string;
  };
  

type ExportItem = {
  name: string;
  type: string;
  functions?: WorkerFunction[];
};

type InvokeType = {
  fun: WorkerFunction;
  instanceName?: string | null;
};

type SelectInvokeProps = {
  exports: ExportItem[];
  invoke: InvokeType | null;
  setInvoke: (invoke: InvokeType) => void;
};

export function SelectInvoke({ exports, invoke, setInvoke }: SelectInvokeProps) {
  // Generate options based on exports
  const options = useMemo(() => {
    return exports.flatMap((item) => {
      if (item.type === "Instance" && item.functions) {
        return item.functions.map((fun) => ({
          label: `${item.name} - ${fun.name}`,
          value: JSON.stringify({ fun, instanceName: item.name }),
        }));
      } else {
        return [{
          label: item.name,
          value: JSON.stringify({ fun: item, instanceName: null }),
        }];
      }
    });
  }, [exports]);

  // Handle value change
  const handleValueChange = (selected: string[]) => {
    if (selected.length > 0) {
      const selectedValue: InvokeType = JSON.parse(selected[0]);
      setInvoke(selectedValue);
    }
  };

  return (
    <div className="max-w-fit">
      <MultiSelect
        selectMode="single"
        options={options}
        buttonType={{ variant: "success", size: "icon_sm" }}
        onValueChange={handleValueChange}
        defaultValue={invoke ? [JSON.stringify(invoke)] : []}
        placeholder="Select function"
        className="flex w-full border min-h-7 h-auto items-center justify-between min-w-[179px]"
        variant="inverted"
        align="end"
        animation={2}
        maxCount={2}
      />
    </div>
  );
}

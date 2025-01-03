import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import {
  CheckIcon,
  XIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { Separator } from "@/components/ui/separator";
import { Button2 as Button } from "./button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";

/**
 * Variants for the multi-select component to handle different styles.
 * Uses class-variance-authority (cva) to define different styles based on "variant" prop.
 */
const multiSelectVariants = cva(
  "m-1 transition ease-in-out delay-150 hover:-translate-y-1 hover:scale-110 duration-300",
  {
    variants: {
      variant: {
        default: "",
        secondary: "",
        destructive: "",
        inverted: "inverted",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
);

/**
 * Props for MultiSelect component
 */
interface MultiSelectProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof multiSelectVariants> {
  /**
   * An array of option objects to be displayed in the multi-select component.
   * Each option object has a label, value, and an optional icon.
   */
  options: {
    /** The text to display for the option. */
    label: string;
    /** The unique value associated with the option. */
    value: string;
    /** Optional icon component to display alongside the option. */
    icon?: React.ComponentType<{ className?: string }>;
  }[];

  /**
   * Callback function triggered when the selected values change.
   * Receives an array of the new selected values.
   */
  onValueChange: (value: string[]) => void;

  /** The default selected values when the component mounts. */
  defaultValue?: string[];

  /**
   * Placeholder text to be displayed when no values are selected.
   * Optional, defaults to "Select options".
   */
  placeholder?: string;

  /**
   * Animation duration in seconds for the visual effects (e.g., bouncing badges).
   * Optional, defaults to 0 (no animation).
   */
  animation?: number;

  /**
   * Maximum number of items to display. Extra selected items will be summarized.
   * Optional, defaults to 3.
   */
  maxCount?: number;

  /**
   * The modality of the popover. When set to true, interaction with outside elements
   * will be disabled and only popover content will be visible to screen readers.
   * Optional, defaults to false.
   */
  modalPopover?: boolean;

  /**
   * If true, renders the multi-select component as a child of another component.
   * Optional, defaults to false.
   */
  asChild?: boolean;

  /**
   * Additional class names to apply custom styles to the multi-select component.
   * Optional, can be used to add custom styles.
   */
  className?: string;

  /** Mode of selection: "single" or "multi". Defaults to "multi". */
  selectMode?: "single" | "multi";

  dropdownType?: string;
}

export const MultiSelect = React.forwardRef<
  HTMLButtonElement,
  MultiSelectProps
>(
  (
    {
      options,
      onValueChange,
      variant,
      defaultValue = [],
      placeholder = "Select options",
      animation = 0,
      maxCount = 2,
      modalPopover = false,
      asChild = false,
      className,
      selectMode = "multi",
      dropdownType,
      ...props
    },
    ref
  ) => {
    const [selectedValues, setSelectedValues] =
      React.useState<string[]>([]);
    const [isPopoverOpen, setIsPopoverOpen] = React.useState(false);

    React.useEffect(()=>{
      setSelectedValues(defaultValue)
    }, [defaultValue?.toString()])

    const handleInputKeyDown = (
      event: React.KeyboardEvent<HTMLInputElement>
    ) => {
      if (event.key === "Enter") {
        setIsPopoverOpen(true);
      } else if (event.key === "Backspace" && !event.currentTarget.value) {
        const newSelectedValues = [...selectedValues];
        newSelectedValues.pop();
        setSelectedValues(newSelectedValues);
        onValueChange(newSelectedValues);
      }
    };

    const toggleOption = (option: string) => {
      let newSelectedValues;
      if (selectMode === "single") {
        newSelectedValues = [option];
      } else {
        newSelectedValues = selectedValues.includes(option)
          ? selectedValues.filter((value) => value !== option)
          : [...selectedValues, option];
      }
      setSelectedValues(newSelectedValues);
      onValueChange(selectMode === "single" ? [newSelectedValues[0]] : newSelectedValues);
    };

    const handleClear = () => {
      setSelectedValues([]);
      onValueChange([]);
    };

    const handleTogglePopover = () => {
      setIsPopoverOpen((prev) => !prev);
    };

    const toggleAll = () => {
      if (selectedValues.length === options.length) {
        handleClear();
      } else {
        const allValues = options.map((option) => option.value);
        setSelectedValues(allValues);
        onValueChange(allValues);
      }
    };

    return (
      <Popover
        open={isPopoverOpen}
        onOpenChange={setIsPopoverOpen}
        modal={modalPopover}
      >
        <div className="flex">
          <PopoverTrigger asChild className="min-w-36">
            {/* { dropdownType!="icon" &&  */}
            <Button
              variant="dropdown"
              size="default"
              ref={ref}
              {...props}
              onClick={handleTogglePopover}
              className={cn(
                "flex w-full p-0 border min-h-10 h-auto items-center justify-between pl-3 rounded-l-full",
                className
              )}
            >
              <div className="flex justify-between items-center w-full">
                {selectedValues.length > 0 ? (
                  <div className="flex flex-wrap">
                    {selectedValues.slice(0, 1).map((value, index) => {
                      const option = options.find((o) => o.value === value);
                      return (
                        <span
                          key={value}
                          className="text-xs text-foreground mx-1 font-light"
                        >
                          {option?.label}
                          {index < 1 && selectedValues.length >= 1}
                        </span>
                      );
                    })}
                    {selectedValues.length > 1 && (
                      <span className="text-xs mx-1 text-foreground font-light">
                        ... {selectedValues.length - 1} more
                      </span>
                    )}
                  </div>
                ) : (
                  <span className="text-xs mx-1 text-muted-foreground font-light">
                    {placeholder}
                  </span>
                )}
              </div>
            </Button>
            {/* {dropdownType=="icon" && <Button variant="success" size="icon_sm">version</Button>} */}
          </PopoverTrigger>
    
          <div
            className={`${
              selectedValues.length >= 1
                ? "dark:hover:bg-[#333] hover:bg-[#e9e7e7] cursor-pointer"
                : "border dark:border-[#444] dark:text-[#444]"
            } text-muted-foreground border border-l-0  py-2 px-3 flex justify-center items-center rounded-r-full`}
            onClick={(event) => {
              event.stopPropagation();
              handleClear();
            }}
          >
            <XIcon size={16} />
          </div>
        </div>

        <PopoverContent
          className="w-auto p-0"
          align="start"
          onEscapeKeyDown={() => setIsPopoverOpen(false)}
        >
          <Command>
            <CommandInput
              placeholder="Search..."
              onKeyDown={handleInputKeyDown}
            />
            <CommandList>
              <CommandEmpty>No results found.</CommandEmpty>
              <CommandGroup>
                {options.map((option) => {
                  const isSelected = selectedValues.includes(option.value);
                  return (
                    <CommandItem
                      key={option.value}
                      onSelect={() => toggleOption(option.value)}
                      className="cursor-pointer"
                    >
                      {isSelected ? (
                        <CheckIcon className="h-4 w-4 text-foreground" />
                      ) : (
                        <div className="w-4 h-4" />
                      )}
                      {option.icon && (
                        <option.icon className="mr-2 h-4 w-4 text-muted-foreground" />
                      )}
                      <span>{option.label}</span>
                    </CommandItem>
                  );
                })}
              </CommandGroup>
              <CommandSeparator />
            {selectMode==="multi" && <CommandGroup>
              <div className="flex items-center justify-between">
                <>
                  <CommandItem
                    onSelect={handleClear}
                    className="flex-1 justify-center cursor-pointer"
                  >
                    Clear
                  </CommandItem>
                  <Separator
                    orientation="vertical"
                    className="flex min-h-6 h-full"
                  />
                </>
                <CommandItem
                  key="all"
                  onSelect={toggleAll}
                  className="flex-1 justify-center cursor-pointer"
                >
                  Select All
                </CommandItem>
              </div>
            </CommandGroup>}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    );
  }
);

MultiSelect.displayName = "MultiSelect";

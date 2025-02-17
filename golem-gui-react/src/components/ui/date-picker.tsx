import * as React from "react"
import { format } from "date-fns"
import { Calendar as CalendarIcon } from "lucide-react"

import { cn } from "@lib/utils"
import { Button2 as Button } from "@ui/button"
import { Calendar } from "@ui/calendar"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@ui/popover"
import { useEffect } from "react"

export function DatePicker({handleChange, defaultValue, label}:{handleChange?:(date?:Date)=>void, defaultValue?:Date, label?:string}) {
  const [date, setDate] = React.useState<Date>()

  useEffect(()=>{
    if(defaultValue){
      setDate(new Date(defaultValue));
    }
  }, [defaultValue])

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant={"outline"}
          className={cn(
            "w-[220px] justify-start text-left font-normal border border-border rounded-full",
            !date && "text-muted-foreground rounded-full"
          )}
        >
          <CalendarIcon className="mr-2 h-4 w-4" />
          {date ? format(date, "PPP") : <span>{label || "Pick a date"}</span>}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-auto p-0">
        <Calendar
          mode="single"
          selected={date}
          onSelect={(date)=>{
            handleChange?.(date)
            setDate(date)
          }}
          initialFocus
        />
      </PopoverContent>
    </Popover>
  )
}

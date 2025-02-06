import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle,
    AlertDialogTrigger,
  } from "@/components/ui/alert-dialog"
  import { Button2 as Button } from "@/components/ui/button"
import { Trash } from "lucide-react"
import React from "react"


  
  export function AlertDialogDemo({onSubmit, paragraph, child}: {onSubmit:(e:any)=>void, paragraph:string, child:React.ReactNode}) {
    return (
      <AlertDialog>
        <AlertDialogTrigger asChild>
          {child}
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Are you absolutely sure?</AlertDialogTitle>
            <AlertDialogDescription>
              {/* This action cannot be undone. This will permanently delete your
              account and remove your data from our servers. */}
              {paragraph}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onSubmit}>Continue</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    )
  }
  
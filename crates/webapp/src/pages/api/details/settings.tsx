/* eslint-disable @typescript-eslint/no-unused-vars */
import * as React from "react";
import { useParams } from 'react-router-dom';
import { useToast } from "@/hooks/use-toast"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import APILeftNav from './APILeftNav';


export default function APISettings() {
  const { toast } = useToast()
  const [version, setVersion] = React.useState("0.1.0")
  const [showConfirmDialog, setShowConfirmDialog] = React.useState(false)
  const [showConfirmAllDialog, setShowConfirmAllDialog] = React.useState(false)
  const [isDeleting, setIsDeleting] = React.useState(false);
  const { apiName } = useParams();


  const handleDeleteVersion = async () => {
    setIsDeleting(true)
    try {
      // Simulate API call
      await new Promise(resolve => setTimeout(resolve, 1000))
      
      toast({
        title: "Version deleted",
        description: `API version ${version} has been deleted successfully.`,
      })
      setShowConfirmDialog(false)
    } catch (error) {
      toast({
        variant: "destructive",
        title: "Error",
        description: "Failed to delete the API version. Please try again.",
      })
    } finally {
      setIsDeleting(false)
    }
  }

  const handleDeleteAll = async () => {
    setIsDeleting(true)
    try {
      // Simulate API call
      await new Promise(resolve => setTimeout(resolve, 1000))
      
      toast({
        title: "All versions deleted",
        description: "All API versions have been deleted successfully.",
      })
      setShowConfirmAllDialog(false)
    } catch (error) {
      toast({
        variant: "destructive",
        title: "Error",
        description: "Failed to delete all API versions. Please try again.",
      })
    } finally {
      setIsDeleting(false)
    }
  }

  return (
    <div className="flex">
    <APILeftNav />
    <div className="flex-1 p-8">
    <div className="flex items-center justify-between mb-8">
        <div className="grid grid-cols-2 gap-4">
          <h1 className="text-2xl font-semibold mb-2">{apiName}</h1>
          <div className="flex items-center gap-2">
            <span className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border border-primary-border w-fit font-mono">0.1.0</span>
          </div>
        </div>
      </div>
    <div className="max-w-4xl mx-auto p-6">
      <h1 className="text-3xl font-semibold mb-2">API Settings</h1>
      <p className="text-gray-500 text-lg mb-8">Manage your API settings</p>

      <div className="border border-red-100 rounded-lg bg-red-50/50 p-6">
        <h2 className="text-2xl font-semibold mb-4">Danger Zone</h2>
        <p className="text-gray-600 mb-8">Proceed with caution.</p>

        <div className="space-y-8">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-xl font-semibold mb-2">Delete API Version {version}</h3>
              <p className="text-gray-600">
                Once you delete an API, there is no going back. Please be certain.
              </p>
            </div>
            <Button 
              variant="outline" 
              className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
              onClick={() => setShowConfirmDialog(true)}
            >
              Delete Version {version}
            </Button>
          </div>

          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-xl font-semibold mb-2">Delete all API Versions</h3>
              <p className="text-gray-600">
                Once you delete all API versions, there is no going back. Please be certain.
              </p>
            </div>
            <Button 
              variant="outline" 
              className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
              onClick={() => setShowConfirmAllDialog(true)}
            >
              Delete All Versions
            </Button>
          </div>
        </div>
      </div>

      {/* Confirmation Dialog for Single Version Delete */}
      <Dialog open={showConfirmDialog} onOpenChange={setShowConfirmDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Are you sure you want to delete?</DialogTitle>
            <DialogDescription>
              This action cannot be undone. This will permanently delete API version {version}.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowConfirmDialog(false)}
              disabled={isDeleting}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={handleDeleteVersion}
              disabled={isDeleting}
            >
              {isDeleting ? "Deleting..." : "Yes, delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Confirmation Dialog for Delete All */}
      <Dialog open={showConfirmAllDialog} onOpenChange={setShowConfirmAllDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Are you sure you want to delete all versions?</DialogTitle>
            <DialogDescription>
              This action cannot be undone. This will permanently delete all API versions
              and remove all associated data.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowConfirmAllDialog(false)}
              disabled={isDeleting}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={handleDeleteAll}
              disabled={isDeleting}
            >
              {isDeleting ? "Deleting..." : "Yes, delete all"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
    </div>
    </div>
  )
}


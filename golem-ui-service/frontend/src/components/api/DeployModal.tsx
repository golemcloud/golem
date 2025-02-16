import * as Yup from 'yup';

import { AlertCircle, Loader2 } from "lucide-react";
import { ExternalLink, Globe, Server, Upload, X } from "lucide-react";
import { Form, Formik } from 'formik';

import { ApiDefinition } from '../../types/api';
import FormInput from '../shared/FormInput';
import toast from 'react-hot-toast';
import { useCreateDeployment } from '../../api/api-definitions';

// Validation schema
const deploymentSchema = Yup.object().shape({
  host: Yup.string()
    .required('Host is required')
    .test('valid-host', 'Invalid host format', function (value) {
      if (!value) return false;

      // Allow localhost with optional port
      if (value.startsWith('localhost')) {
        const parts = value.split(':');
        if (parts.length === 1) return true;
        if (parts.length === 2) {
          const port = parseInt(parts[1]);
          return !isNaN(port) && port >= 0 && port <= 65535;
        }
        return false;
      }

      // Regular domain validation
      const domainRegex = /^[a-zA-Z0-9][a-zA-Z0-9-]{1,61}[a-zA-Z0-9](?:\.[a-zA-Z]{2,})+$/;
      return domainRegex.test(value);
    }),
  subdomain: Yup.string()
    .matches(/^[a-z0-9-]*$/, 'Subdomain can only contain lowercase letters, numbers, and hyphens')
    .max(63, 'Subdomain cannot exceed 63 characters')
});

interface DeployModalProps {
  isOpen: boolean;
  onClose: () => void;
  apiDefinition: ApiDefinition;
}


export const DeployModal = ({
  isOpen,
  onClose,
  apiDefinition,
}: DeployModalProps) => {
  const createDeployment = useCreateDeployment();

  const handleDeploy = async (values: { host: string; subdomain: string }) => {
    try {
      await createDeployment.mutateAsync({
        apiDefinitions: [{
          id: apiDefinition.id,
          version: apiDefinition.version,
        }],
        site: {
          host: values.host.toLowerCase().trim(),
          subdomain: values.subdomain.toLowerCase().trim() || undefined,
        },
      });
      toast.success("API deployed successfully");
      onClose();
    } catch (error) {
      toast.error("Failed to deploy API");
      console.error(error);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-lg p-6 max-w-md w-full">
        <div className="flex justify-between items-start mb-6">
          <div>
            <h2 className="text-xl font-semibold flex items-center gap-2">
              <Upload className="h-5 w-5 text-success" />
              Deploy API
            </h2>
            <p className="text-sm text-muted-foreground mt-1">
              {apiDefinition.id} v{apiDefinition.version}
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground transition-colors"
          >
            <X size={20} />
          </button>
        </div>

        <Formik
          initialValues={{
            host: 'localhost:9006',
            subdomain: ''
          }}
          validationSchema={deploymentSchema}
          onSubmit={handleDeploy}
        >
          {({ errors, touched, values,handleChange, isSubmitting }) => (
            <Form className="space-y-6">
              <div className="bg-card/50 rounded-lg p-4">
                <h3 className="text-sm font-medium flex items-center gap-2 mb-2">
                  <Server className="h-4 w-4 text-primary" />
                  Deployment Configuration
                </h3>
                <p className="text-sm text-muted-foreground">
                  Configure where your API will be deployed. The host should be a
                  valid domain name or localhost.
                </p>
              </div>

              <div className="space-y-4">
                <FormInput
                  label="Host"
                  name="host"
                  icon={Globe}
                  value={values.host}
                  onChange={handleChange}
                  placeholder="localhost:9006 or api.example.com"
                />
                {touched.host && errors.host && (
                  <div className="text-sm text-destructive flex items-center gap-1 mt-1">
                    <AlertCircle size={14} />
                    <span>{errors.host}</span>
                  </div>
                )}

                <FormInput
                  label="Subdomain"
                  name="subdomain"
                  icon={ExternalLink}
                  value={values.subdomain}
                  onChange={handleChange}
                  optional
                  placeholder="v1"
                  hint="Use subdomains to organize different versions or environments"
                />
                {touched.subdomain && errors.subdomain && (
                  <div className="text-sm text-destructive flex items-center gap-1 mt-1">
                    <AlertCircle size={14} />
                    <span>{errors.subdomain}</span>
                  </div>
                )}
              </div>

              {/* Preview */}
              {(values.host || values.subdomain) && (
                <div className="bg-card/60 rounded-lg p-3 font-mono text-sm">
                  <div className="text-muted-foreground mb-1">Preview URL:</div>
                  <div className="text-success">
                    http://{values.subdomain ? `${values.subdomain}.` : ""}
                    {values.host}
                  </div>
                </div>
              )}

              <div className="flex justify-end space-x-3 mt-6">
                <button
                  type="button"
                  onClick={onClose}
                  className="px-4 py-2 text-sm bg-card/80 rounded-md 
                    hover:bg-card/60 transition-colors"
                  disabled={isSubmitting}
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={isSubmitting || Object.keys(errors).length > 0}
                  className="px-4 py-2 text-sm bg-success text-success-foreground 
                    rounded-md hover:bg-success/90 disabled:opacity-50 
                    flex items-center gap-2 transition-colors"
                >
                  {isSubmitting ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      <span>Deploying...</span>
                    </>
                  ) : (
                    <>
                      <Upload size={16} />
                      <span>Deploy API</span>
                    </>
                  )}
                </button>
              </div>
            </Form>
          )}
        </Formik>
      </div>
    </div>
  );
};

export default DeployModal;
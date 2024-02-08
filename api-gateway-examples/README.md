## Integrating Golem with existing API Gateways (Document in progress)

Once we are able to deploy our Golem service, we can integrate it with existing API Gateways. This is a common use case for Golem, as it allows us to leverage the existing infrastructure and security features of the API Gateway, while still being able to use Golem for the actual processing of the requests.

In order for this to work, we introduce a worker-bridge, that act as a bridge between your preferred API Gateway and the actual
worker instance that's running in Golem. 

We define the endpoint definitions and send it to worker bridge. This definition is merely the set of endpoints,
and the actual function that needs to be executed by the particular worker instance to serve the endpoint

Here is an example:

```json

{
  "id": "my-api",
  "version": "0.0.1",
  "routes": [
    {
      "method": "Get",
      "path": "/",
      "binding": {
        "type": "wit-worker",
        "template": "c467b83d-cb27-4296-b48a-ee85114cdb71",
        "workerId": "myworker2",
        "functionName": "golem:it/api/get-cart-contents",
        "functionParams": []
      }
    }
  ]
}


```

Registration of this endpoint definition is pretty simple. The details of how much you can configure can be discussed later.
Currently, we are just focusing on the basic registration of the endpoint definition.

```scala
cd api-gateway-examples
curl -X PUT http://localhost:9005/v1/api/definitions -H "Content-Type: application/json"  -d @endpoint_definition.json
```

## Integration with Tyk API Gateway

Once you register this Endpoint definitions that relates to a specific worker and function, you can now use API Gateway
to forward request to the worker bridge. Let's say we choose Tyk as the API Gateway. A typical API definition required by Tyk is

```
{
    "name": "Tyk Test Keyless API",
    "api_id": "keyless",
    "org_id": "default",
    "definition": {
        "location": "header",
        "key": "version"
    },
    "use_keyless": true,
    "version_data": {
        "not_versioned": true,
        "versions": {
            "Default": {
                "name": "Default"
            }
        }
    },
    "custom_middleware": {
        "pre": [
          {
            "name": "testJSVMData",
            "path": "./middleware/injectHeader.js",
            "require_session": false,
            "raw_body_only": false
          }
        ]
  },
    "driver": "otto",
    "proxy": {
        "listen_path": "/keyless-test2/",
        "target_url": "http://192.168.18.202:9006/",
        "strip_listen_path": true
    }
}

```
See the target_url. This is the URL of the worker bridge that is ready to serve your custom requests. The worker bridge will then forward the request to the actual worker instance.
However, inorder for this to work, we need to set a middleware that adds an extra header called "X-API-Definition-Id" whose
value is the id of the endpoint definition that we registered with the worker bridge. In our example, it is "my-api".
This is how the worker bridge knows which endpoints it needs to serve for the requests forwarded from API Gateway.

Below given are the step by step instructions to follow to try all of this in Local.


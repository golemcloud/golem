## Integrating Golem with existing API Gateways (Document in progress)

This document covers how to expose your Golem service to the outside world using API endpoints (including integration with external API Gateways)
It's worth reading this document before you wrap your worker functionalities as a backend service exposing API endpoints
to outside world, because you may get this for free.

In order for this to work, we have incorporated a functionality into worker-service that can accept API definitions that can map endpoints to workers.
Implementation detail: Internally, a mini (poor man's) gateway functionality is incorporated into worker-service. This is just a detail or the curious ones.

We write an API definition and register with the worker service. 
On a high level, this definition is merely a set of endpoints, and the actual function that needs to be executed by your particular worker instance to serve the endpoint

Registration of this endpoint definition is pretty simple. The details of how much you can configure can be discussed later.
Currently, we are just focusing on the basic registration of the endpoint definition.

## An example, including integration with Tyk API Gateway

Once we are able to deploy our Golem service, we can integrate it with external API Gateways. Here we took Tyk as an example.
This is a common use case for Golem, as it allows us to leverage the existing infrastructure and security features of the API Gateway,
while still being able to use Golem for the actual processing of the requests.

Once you register this Endpoint definitions that relates to a specific worker and function, you can now use API Gateway
to forward request to the worker bridge. Let's say we choose Tyk as the API Gateway. A typical API definition required by Tyk is

Below given are the step by step instructions to follow to try all of this in Local.

### Step 1: Spin up Golem

```bash
# Clone golem-services and spin up all services which includes worker-bridge

cargo build --release --target x86_64-unknown-linux-gnu
docker-compose -f docker-compose-sqlite.yaml up --build
```

### Step 2: Deploy shopping cart example

```bash
cd golem-services
# Note down the template id, say "c467b83d-cb27-4296-b48a-ee85114cdb7"
golem-cli template add --template-name mytemplate test-templates/shopping-cart.wasm

# Note down the worker-name, here it is myworker
golem-cli worker invoke-and-await  --template-name mytemplate --worker-name worker-adam --function golem:it/api/add-item --parameters '[{"product-id" : "hmm", "name" : "hmm" , "price" : 10, "quantity" : 2}]'
```

### Step 3: Register the endpoint definition

Please make sure to use the correct template-id based on the output from `template add` command.
A typical worker bridge endpoint definition looks like this. Please refer to [endpoint_definition.json](endpoint_definition.json) for a complete example.

```bash
{
  "id": "shopping-cart-v1",
  "version": "0.0.2",
  "routes": [
    {
      "method": "Get",
      "path": "/{user-id}/get-cart-contents",
      "binding": {
        "type": "wit-worker",
        "template": "08930752-d868-412f-a608-b834bda159be",
        "workerId": "worker-${request.path.user-id}",
        "functionName": "golem:it/api/get-cart-contents",
        "functionParams": [],
        "response" : {
          "status": "200",
          "body": {
            "name" : "${worker.response[0][0].name}",
            "price" : "${worker.response[0][0].price}",
            "quantity" : "${worker.response[0][0].quantity}"
          },
          "headers": {}
        }
      }
    }
  ]
}


```

```bash
cd api-gateway-examples
# register with worker bridge
# Ensure to make change in template-id in endpoint_definition.json
# Our golem service is accessible through localhost:9881. (It will redirect to the right internal service)
curl -X PUT http://localhost:9881/v1/api/definitions -H "Content-Type: application/json"  -d @endpoint_definition.json

```

Step 4: Install Tyk API gateway

```bash
# In some other location
git clone https://github.com/TykTechnologies/tyk-gateway-docker
cd tyk-gateway-docker
docker-compose up
```

Register the API definition with Tyk. We are OAS API Definition of Tyk. You can read more about it in Tyk documentation

# Tyk supports Classic API definition and OAS Api definition (limitations)
```json
curl --location --request POST 'http://localhost:8080/tyk/apis' \
--header 'x-tyk-authorization: foo' \
--header 'Content-Type: text/plain' \
--data-raw \
'{
    "name": "API to showcase integration with Golem",
    "api_id": "shopping-cart-tyk",
    "org_id": "default",
    "definition": {
        "location": "header",
        "key": "version"
    },
    "use_keyless": true, 
     "cache_options": {
       "enable_cache": true,
       "cache_timeout": 1,
       "cache_all_safe_requests": true,
       "cache_response_codes": [200]
    },   
            
    "version_data": {
        "not_versioned": true,
        "versions": {
            "Default": {
                "name": "Default",
                "global_headers": {
                    "x-golem-api-definition-id":"shopping-cart-v1",
                    "x-golem-api-definition-version": "0.0.3"
                }
            }
        }
    },
    "driver": "otto",
    "proxy": {
        "listen_path": "/v10",
        "target_url": "http://169.254.141.101:9006/",
        "strip_listen_path": true
    }
}'

```
Reload the gateway, otherwise the API is not deployed with Tyk yet, so this is an important step.

```bash
curl -H "x-tyk-authorization: foo" -s http://localhost:8080/tyk/reload/group

```


### Important aspects
* Anything with listen_path /v10 will be forwarded to the worker bridge.
* Tyk injects x-golem-api-definition-id and x-golem-api-definition-version headers to the request, which is the id and version of the endpoint definition that we registered with the worker bridge
* With docker set up, we have 2 different docker networks running. Therefore, the IP of the worker-bridge is the IP address of the machine (and not localhost) http://192.168.18.101:9006/.
* The target URL is url of the worker bridge that is ready to serve your custom requests. 
* Worker bridge is already registered with the API definition ID shopping-cart. If the worker bridge is not registered with the correct API definition, it will return something like the following

```
 API request definition id shfddfopping-cart not found%
```

* Caching is enabled in Tyk API Gateway, just to show-case, we get these features of external API gateways for free.

With all this in place, you can now make requests to the API Gateway and see the worker bridge forwarding the requests to the actual worker instance.


## Let's try out
```bash


curl -X GET http://localhost:8080/v10/adam/get-cart-contents
 
{"name":"hmm","price":10.0,"quantity":2}

```

## Alternate and easier workflow using OpenAPI Spec


If we have an OpenAPI spec of the backend services, with a few additional information relating to worker-bridge and Tyk, 
we can use the same to register with worker-bridge and API Gateway.

### Step 1: Registration with worker-bridge

After creating a template and a worker with golem-services,

```bash

cd api-gateway-examples

# Refer to open_api.json. servers section is for Tyk and worker-bridge section is for worker-bridge
curl -X PUT http://localhost:9881/v1/api/definitions/oas -H "Content-Type: application/json" --data-binary "@open_api.json"

```

### Step 2: Registration with Tyk

```bash

curl -X POST http://localhost:8080/tyk/apis/oas/import --header 'x-tyk-authorization: foo' --header 'Content-Type: text/plain' -d @open_api.json

# then reload
curl -H "x-tyk-authorization: foo" -s http://localhost:8080/tyk/reload/group


```

### Step 3: Try out

```bash

# TODO; Note, with using OAS API Definition in Tyk - harder to add headers to the request without a management console, therefore explicitly passing it here for demo purpose 
# In real world, the header is injected by Tyk
curl -X GET http://localhost:8080/adam/get-cart-contents -H "x-golem-api-definition-id: shopping-cart-v2" -H "x-golem-api-definition-version: 0.0.3"

```

## Why do we need to upload definition to both worker-bridge and API Gateway?
Here is an excerpt from Tyk's documentation on a similar aspect:

> Crucially, the user’s API service remains unaware of the Tyk Gateway’s processing layer, responding to incoming requests as in direct client-to-service communication. It implements the API endpoints, resources and methods providing the service’s functionality. It can also have its own OpenAPI document to describe and document itself (which is essentially also another name for API definition)

A backend service can have its own API definition or documentation. And at the same time, the reverse proxy configurations
may have its own documentation. Therefore it is not by surprise, we also have a 2 layer documentation for services
backed by API Gateway.


## How does worker bridge know which API definition to pick for a given endpoint?

*x-golem-api-definition-id* and *x-golem-api-definition-version* are the headers that are injected by Tyk to the request.

By injecting these headers to every request, worker bridge can lookup the corresponding API definition and serve the request.
It is the responsibility of whoever managing the API Gateway (Tyk in this case) to make sure that every request is configured to inject
these headers.

Please note that, if you are using open-api spec in registering with worker-bridge, x-golem-api-definition-version is _NOT_
the version of the open-api spec. You may have to change the open-api spec that you upload to Tyk (with extra caching or authorisation for example),
and still point to the same version of the API definition in worker-bridge. It is upto the user whether or not 
they keep the version of the open-api spec and the version of the API definition in worker-bridge to be same. Conceptually,
they don't necessarily be the same.


## What next?

### Document Generation aspects

* Users can use their Open API spec that can be imported to worker bridge, with an obvious interface to specify the worker name and function name.. Sometimes there may not be any transformations that everything else is mere defaults
* If they want (not mandatory) They can make use of the same Open API spec to upload to API Gateway if they want to configure per-endpoint. Example: We allow 10000 requests per second for Get cart contents, but 1000 for posting. Otherwise (my draft PR) all requests to API Gateway is forwarded as is to worker bridge

We can flip this thinking too

* Users write the worker bridge API definition that is even more powerful with respect to a backend service, especially with transformations using Expr language
* They can generate Open API spec (probably with some challenging part in details) from this, and if they want (not mandatory) can make use of it to upload to their preferred API gateways and achieve the same advantages mentioned above in the second point. Otherwise all requests to API Gateway is forwarded to worker bridge
Given we already achieved what we discussed on Wednesday in my draft PR, may be its not a bad idea to discuss/validate some of these points (


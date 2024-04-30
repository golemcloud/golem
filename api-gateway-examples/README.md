## Integrating Golem with existing API Gateways (Document in progress)

To expose your Golem service to the outside world using API endpoints, we've integrated functionality into the worker-service that accepts API definitions, effectively mapping endpoints to workers. This implementation incorporates a mini gateway functionality within the worker-service.

Here's a brief overview of how it works:

* Write an API definition specifying endpoints and the corresponding worker functions.
* Register this API definition with the worker-service.
* Now, the worker-service can function as a mini-gateway, capable of integrating with external API gateways if necessary.

The API definition essentially comprises a set of endpoints, each associated with the function to be executed by a specific worker instance to serve the endpoint.

The registration process for this endpoint definition is straightforward. Further configuration details can be discussed later as needed.

## An example, including integration with an external API Gateway

After deploying our Golem service, integration with external API Gateways, like Tyk, becomes straightforward. 
This setup enables us to utilize the robust infrastructure and security features provided by the API Gateway while leveraging Golem for request processing.

Once you register this API definitions with worker-service that relates to a specific worker and function, 
you can now use external API Gateway (which will require its own API definition, and possibly we can reuse it - discussed later)
to forward request to the worker service. Let's say we choose Tyk as the API Gateway.

### Step 1: Spin up Golem

```bash
# Clone golem-services and spin up all services which includes worker-bridge

cargo build --release --target x86_64-unknown-linux-gnu
docker-compose -f docker-compose-sqlite.yaml up --build
```

### Step 2: Deploy shopping cart example

```bash
cd golem-services
# Note down the component id, say "c467b83d-cb27-4296-b48a-ee85114cdb7"
golem-cli component add --component-name mytemplate test-components/shopping-cart.wasm

# Note down the worker-name, here it is myworker
golem-cli worker invoke-and-await  --component-name mytemplate --worker-name worker-adam --function golem:it/api/add-item --parameters '[{"product-id" : "hmm", "name" : "hmm" , "price" : 10, "quantity" : 2}]'
```

### Step 3: Register the endpoint definitions with worker-service

Please make sure to use the correct template-id based on the output from `template add` command.
A typical worker bridge endpoint definition looks like this. Please refer to this [example](worker_service_api_definition.json).

```bash
cd api-gateway-examples
# register with worker bridge
# Ensure to make change in component-id in worker_service_api_definition.json
# Our golem service is accessible through localhost:9881. (It will redirect to the right internal service)
curl -X PUT http://localhost:9881/v1/api/definitions -H "Content-Type: application/json"  -d @worker_service_api_definition.json

```

Step 4: Install Tyk API gateway

```bash
# In some other location
git clone https://github.com/TykTechnologies/tyk-gateway-docker
cd tyk-gateway-docker
docker-compose up
```

Configure Tyk with it's [API definition](reusable_open_api_definition.json) that specify things like caching, authorisation etc. 
Tyk supports Classic API definition and OAS Api definition (with some limitations). Here is an example of how to configure Tyk with Classic API definition.
Make sure to update the IP Address of your machine for target URL.

```json
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
Reload the Tyk API Gateway, otherwise the API is not deployed with Tyk yet, so this is an important step.
Note that, if you are encountering issues following these steps, please refer to Tyk documentations.

```bash
curl -H "x-tyk-authorization: foo" -s http://localhost:8080/tyk/reload/group

```

### Important aspects
* Anything with listen_path /v10 will be forwarded to the worker service.
* Tyk injects x-golem-api-definition-id and x-golem-api-definition-version headers to the request, which is the id and version of the [API-Definition](worker_service_api_definition.json) that we registered with the worker bridge
* With docker set up, we have 2 different docker networks running. Therefore, the IP of the worker-service is the IP address of the machine (and not localhost) http://192.168.18.101:9006/
* The target URL is url of the worker service that is ready to serve your custom requests. 
* Worker service is already registered with the API definition ID shopping-cart. If the worker service is not registered with the correct API definition, it will return something like the following

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


If we have an OpenAPI spec of the backend services, with a few additional information relating to worker-service and Tyk, 
we can use the same to register with worker-service and API Gateway.

### Step 1: Registration with worker-bridge

After creating a template and a worker with golem-services,

```bash

cd api-gateway-examples

curl -X PUT http://localhost:9881/v1/api/definitions/oas -H "Content-Type: application/json" --data-binary "@reusable_open_api_definition.json"

```

### Step 2: Registration with Tyk

```bash

curl -X POST http://localhost:8080/tyk/apis/oas/import --header 'x-tyk-authorization: foo' --header 'Content-Type: text/plain' -d @reusable_open_api_definition.json

# then reload
curl -H "x-tyk-authorization: foo" -s http://localhost:8080/tyk/reload/group


```

### Step 3: Try out

```bash

# TODO; Note, with using OAS API Definition in Tyk - harder to add headers to the request without a management console, therefore explicitly passing it here for demo purpose 
# In real world, the header is injected by Tyk
curl -X GET http://localhost:8080/adam/get-cart-contents -H "x-golem-api-definition-id: shopping-cart-v2" -H "x-golem-api-definition-version: 0.0.3"

```


## Avoiding Tyk or other API gateway

If you want to directly hit the worker service, all you need to do is hit the worker-service that runs in port ${WORKER_SERVICE_CUSTOM_REQUEST_PORT} (refer .env file used docker-compose) dir

If WORKER_SERVICE_CUSTOM_REQUEST_PORT is 9006, that means you can directly hit the worker service at http://localhost:9006

```bash

curl -X GET http://localhost:9006/adam/get-cart-contents -H "x-golem-api-definition-id: shopping-cart-v2" -H "x-golem-api-definition-version: 0.0.3"

```

However, in this case, you need to explicitly pass the headers unlike the workflow with Tyk where it was configured to be automatically injected.

## Why do we need to upload definition to both worker-service and API Gateway?

This is a normal scenario backend service (in our case, worker service) can have its own API definition or documentation. 
And at the same time, the reverse proxy configurations (Tyk or any external API) will need its own documentation. 

Here is an excerpt from Tyk's documentation on a similar aspect:

> Crucially, the user’s API service remains unaware of the Tyk Gateway’s processing layer, responding to incoming requests as in direct client-to-service communication. It implements the API endpoints, resources and methods providing the service’s functionality. It can also have its own OpenAPI document to describe and document itself (which is essentially also another name for API definition)


## How does worker service know which API definition to pick for a given endpoint?

*x-golem-api-definition-id* and *x-golem-api-definition-version* are the headers that are injected by Tyk to the request,
which will allow worker-service to look up the right API definition (so that it knows which worker function to invoke) for a given request.

It is the responsibility of whoever managing the API Gateway (Tyk in this case) to make sure that every request is configured to inject
the above headers.

Please note that, if you are using open-api spec in registering with worker-service, x-golem-api-definition-version is _NOT_
the version of the open-api spec. You may have to change the open-api spec that you upload to Tyk (with extra caching or authorisation for example),
and still point to the same version of the API definition in worker-service. It is upto the user whether or not 
they keep the version of the open-api spec and the version of the API definition in worker-service to be same. Conceptually,
they don't necessarily be the same.

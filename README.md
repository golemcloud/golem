# Golem

![Golem Logo](golem-logo-black.jpg)

This repository contains Golem - a set of services enable you to run WebAssembly components in a distributed cloud environment.

See [Golem Cloud](https://golem.cloud) for more information.

## Getting Started

It is possible to start using Golem locally by using our published Docker containers. Please refer to the document link below on how to get golem OSS running using docker.
https://learn.golem.cloud/docs/quickstart#setting-up-golem

Once you have Golem running locally, you can use `golem-cli` to interact with Golem services.

```bash

cargo install golem-cli

# component is your compiled code written in Rust, C, etc
# https://learn.golem.cloud/docs/building-templates helps you write some code and create a component - as an example
golem-cli component add --compnent-name <component-name> <location-to-component-file> 

# Now we need a worker corresponding from component, that can execute one of the functions in component
# If worker doesn't exist, it is created on the fly whey you invoke a function in component
golem-cli worker invoke-and-await --component-name <component-name> --worker-name <worker-name> --function golem:it/api.{add-item} --parameters '[{"product-id" : "foo", "name" : "foo" , "price" : 10, "quantity" : 1}]'

```

Internally, it is as simple as `golem-cli` using `golem-client` sending requests to Golem Services hosted in Docker container.
Therefore, you can see what's going on and troubleshoot things by inspecting docker containers.

```


+-----------------------+         +-----------------------+
|                       |         |                       |
|  Use golem-cli        |  --->   |  Golem Services       |
|                       |         |  hosted in            |
|  commands             |         |  Docker container     |
|  (Send Requests)      |         |                       |
+-----------------------+         +-----------------------+

```


## Compiling Golem locally
Find details in the [contribution guide](CONTRIBUTING.md) about how to compile the Golem services locally.

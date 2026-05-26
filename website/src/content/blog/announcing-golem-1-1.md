---
title: "Announcing Golem 1.1"
author: "John A. De Goes"
slug: "announcing-golem-1-1"
originalUrl: "https://golem.cloud/post/announcing-golem-1-1"
date: "2024-12-09"
# date sourced from Golem v1.1.0 release published_at 2024-12-09T12:07:46Z (api.github.com/repos/golemcloud/golem/releases tag v1.1.0); site-deploy timestamp at first wayback snapshot of post is also "Mon Dec 09 2024"
---

We're excited to announce the release of Golem 1.1, a major update to our open-source durable computing platform. This release introduces groundbreaking features that make Golem more flexible, developer-friendly, and production-ready than ever before.

## **Software-Defined Reliability with Ephemeral Workers**

The headline feature of Golem 1.1 is the introduction of ephemeral workers, making Golem the first unified computing platform with software-defined reliability. This allows developers to choose the right level of durability for each component of their application:

- Deploy critical business logic as durable workers with Golem's hallmark reliability guarantees
- Use ephemeral workers for stateless operations where high reliability isn't required
- Seamlessly mix both types in the same application while maintaining Golem's strong transactional guarantees

This flexibility enables more cost-effective deployments while preserving reliability where it matters most.

## **Enhanced Developer Experience**

We've made significant improvements to the developer experience:

### **Simplified Worker Communication**

The new application manifest system streamlines worker-to-worker communication with declarative dependencies and improved type handling. This eliminates common pain points around circular dependencies and type duplication.

### **Redesigned Console**

The console interface has been completely revamped with a GitHub-inspired design that reduces navigation complexity from four levels to two, making it more intuitive to manage your applications.

### **RIB Language Improvements**

RIB, our API scripting language, now supports list comprehensions and aggregations, enabling more expressive data transformations. We've also made substantial robustness improvements to the compiler and type system.

### **Single Executable**

Golem now ships as a single executable that runs the entire stack, dramatically simplifying the developer experience for local testing and development.

## **Enterprise-Ready Features**

### **Plugin System**

The new plugin system enables easy extension of Golem's capabilities through:

- Oplog Processors for custom observability integrations
- Component Transformers for modifying components during deployment

We will be launching much more content around plugins as the feature matures.

### **Authentication**

Built-in end-user authentication support for major identity providers including Google, GitHub, Microsoft, and others, with seamless integration into RIB scripts.

### **Enhanced Observability**

New oplog search and enumeration capabilities make it easier to understand and debug your applications, with built-in support for streaming large logs.

### **Production-Ready Gateway**

The worker gateway now includes built-in CORS support and improved authentication handling, making it easier to build production applications.

## **Getting Started**

To get started with Golem 1.1, download the new single executable for your platform and check out our [documentation](https://learn.golem.cloud/) for detailed guides and examples.

We're excited to see what you'll build with these new capabilities. Your feedback and contributions continue to shape Golem's development - please share your experiences and suggestions with our community.

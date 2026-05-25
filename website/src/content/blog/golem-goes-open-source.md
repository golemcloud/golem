---
title: "Golem Goes Open Source"
date: "2024-02-05"
# date sourced from site-deploy timestamp "Mon Feb 05 2024" embedded in first wayback snapshot of post (web.archive.org/web/20240206092108/https://www.golem.cloud/post/golem-goes-open-source); related companion post "golem-goes-open-source-join-our-webinar-on-march-6th" announces a Mar 6 2024 webinar, consistent with an early-Feb publish date
author: "John A. De Goes"
slug: "golem-goes-open-source"
originalUrl: "https://golem.cloud/post/golem-goes-open-source"
---

Six months ago, we announced the Developer Preview release of [Golem Cloud](/blog/unveiling-golem-cloud). Available for trial in our managed offering (but not for production usage), Golem Cloud is pioneering a new way to offer durable execution: as a fully general-purpose computing platform.

Unlike other solutions in the space, [Golem Cloud](https://golem.cloud) lets you take programs written in any language or technology stack, and assuming you can target [WebAssembly](https://webassembly.org/) (which is as simple as `--wasm` in some compiler toolchains), you can execute them unmodified on Golem Cloud.

Unlike mainstream computing platforms like AWS, Golem Cloud executes programs invincibly: regardless of updates to your code, restarts, killed containers, configuration changes, or even hardware failures, Golem Cloud continues to execute your code to completion.

Powered by continuous real-time replication, combined with supervision and rapid failover, Golem Cloud offers an unprecedented degree of reliability for cloud developers, without forcing them to adopt high-reliability patterns like event-sourcing with state machines.

During the six months since the release of Developer Preview, we've seen tremendous excitement about what we are doing. "This will change everything!" was a quote from one excited user (and although we are biased, it's hard for us to disagree!).

As users have experimented with the platform, building early stage applications, we've also received critical feedback about where Golem Cloud needs to go in order to be an unstoppable force for next-generation, highly reliable cloud computing.

In this post, I will outline the near term milestones for Golem, beginning with (perhaps) the biggest announcement of them all.

### Golem Goes 100% Open Source

Even in the Developer Preview, Golem has been an ardent adopter and promoter of open standards. Yet, six months ago when we released the preview, we decided not to open source the core technology.

We needed to learn if Golem would be closer to an alternate, more reliable executor of cloud services, or to a foundational piece of cloud infrastructure, which companies specifically build solutions for (and which cannot usefully be executed apart from Golem).

After six months of feedback, we finally have our answer: Golem is a foundational piece of cloud infrastructure. Even though Golem can execute programs written in any language and any stack (so long as it targets WebAssembly), it is clear that durable computing changes the way you write programs; and, moreover, that WebAssembly is a sufficiently early technology to require flexibility with both language and stack choice (this will change in the future, but it will take time).

If Golem is a foundational piece of cloud infrastructure, then it's clear why Golem **must** be open source:

- **Trust & Longevity**. Companies who build solutions for Golem must have the confidence that they can run their software for decades, which is exactly what they require for Kubernetes, Postgres, Redis, and other key components of modern cloud native applications.
- **Market Leadership**. The lack of durable computing is such a significant pain, many vendors and companies have strong incentives to collaborate on a small number of solutions, rather than duplicate efforts producing a large number of similar solutions. If Golem were not open source, then another solution would be open source, due to these strong incentives, and many vendors and organizations would rally around the open source solution, rather than proprietary versions.
- **Openly Disruptive**. As Max Schireson (former CEO of MongoDB) told me one day over lunch, the cost of requiring developers do something new is open source. How I would phrase this keen insight today is that you can sell software which improves on the same way people do things now; but if you require people do something different, you need to give it away.

Moreover, not only are there critical reasons why Golem must be open source, but there are some pretty awesome benefits, too:

- **Alignment**. For the past 4 years, myself and other founders have spent our time building open source, listening to users, iterating, and building a vibrant and large community around our open source. We learned how to succeed in a fiercely competitive and even hostile market.
- **Security & Correctness**. Open source projects, by their nature, attract a lot of scrutiny, bug reports, and feature requests, and this information can be used to rapidly improve the product. Some edge case bug that might take years to be detected with a closed source solution could be detected in weeks with an open source solution.
- Because of the incentives to collaborate on durable computing technologies, we believe that we can recruit many contributors, who will help accelerate the feature set that Golem offers, and more quickly usher in the next age of high-reliable computing.

For these reasons, we are delighted to announce that Golem has officially been [open sourced](https://github.com/golemcloud)! We encourage developers and organizations interested in durable computing to browse through the repositories, fork, play with, and build applications that run on Golem, with the full confidence that comes with an open source solution.

Open sourcing is just the start of Golem's journey in 2024. Our next stop is nothing less than a 1.0 milestone, which we are aggressively targeting for May 6.

### Looking Toward 1.0

Golem is completely usable for building and deploying applications today. However, as with any early stage product (and, indeed, Golem has not yet had its 1 year birthday), there is still much more work we need to do before we can officially release a 1.0 version.

Broadly speaking, our work ahead can be characterized into the following buckets:

- **Polishing & Hardening**. This bucket of work includes numerous improvements to shard manager (which is the key to clustering); reproducible benchmarking and stress testing; memory caps; multi-cluster support; architectural improvements, which, among other things, will permit any API gateway to sit in front of Golem; and durability audit.
- **Golem Runtime API**. This bucket of work includes the transaction API, which allows developers to customize the transactional semantics of Golem, as well as retry policies, from within their applications; and a configuration API, which can be used to pull configuration information into a Golem application.
- **Platform Improvements**. This bucket of work includes infinite worker history; worker-to-worker communication (including rudimentary permissions); worker enumeration; automatic updates of Golem application components; public purging support; and improvements to the automatic resilience features built into the platform.

This new and improved functionality will place Golem into a highly competitive position, and we expect to see our first production deployments atop 1.0 (if not sooner).

Once we have released the first official version of Golem, we will turn some attention to our cloud hosted version of Golem.

### Golem Cloud 1.0

Our cloud hosted version of Golem, which has always been called Golem Cloud, will build on the open source edition. Layer on multi-tenancy (including a multi-tenant permissioning system), auto-scaling, and an API Gateway, Golem Cloud will also feature improvements to the current version of the Management Console, which lets Golem Cloud users manage their applications.

We expect to offer Golem Cloud 1.0 in an on-premise edition, bundled together with commercial support for Golem (a key requirement for some companies before they put open source into production).

We have no definite timing for Golem Cloud 1.0, but we will work together with early users to ensure that we have the right feature set for a managed, multi-tenant offering.

### Breakout Year

It has been less than a year since me and the two other founders of Golem Cloud discussed and decided to embark on this journey into durable computing. Since then, a number of other players have entered the space, which is a strong signal that we are entering an exciting new era for durable execution.

Between the open sourcing of Golem, the first release version, and improvements to Golem Cloud, I believe this will be a breakout year for durable computing, and for Golem in particular.

I'm very excited to be a part of this exciting new technology, and now with the open sourcing of Golem, I am inviting you to be a part of the technology, too.

As a developer, come experience how easy it is to build and deploy rock solid, invincible services; as a contributor, push Golem forward, or just play around; and as a company, take a brave new step into the future of reliable cloud computing.

I hope that in any case, you will [join us](https://ziverge.zoom.us/webinar/register/WN_KyGE7OMOQe-eEA99QBGXfg) in two weeks for [our webinar](https://ziverge.zoom.us/webinar/register/WN_gEF72meqSm-JcAyxnY5TNA) that walks you through open source Golem and shows you how to get started building and deploying apps using this open source platform.

See you there!

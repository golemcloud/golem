---
title: "How David Flanagan Made Golem Work For Him"
date: "2024-08-15"
# date sourced from site-deploy timestamp "Thu Aug 15 2024" embedded in first wayback snapshot of post (web.archive.org/web/20240816*/https://www.golem.cloud/post/how-david-flanagan-made-golem-work-for-him)
author: "Golem Cloud"
tags: ["Interview", "Web Assembly", "Durable Execution", "Backend Development", "Rawkode Academy"]
slug: "how-david-flanagan-made-golem-work-for-him"
originalUrl: "https://golem.cloud/post/how-david-flanagan-made-golem-work-for-him"
---

We had the wonderful opportunity to interview the incredible David Flanagan about [Rawkode Akademy](https://rawkode.dev/) and how they've utilized Golem.

<iframe allowfullscreen="true" frameborder="0" scrolling="no" src="https://www.youtube.com/embed/EjOpZeW4zwk"></iframe>

#### **Q: Do you mind sharing your name, title, and what you do for Rawkode Academy?**

**David:** I'm David Flanagan. I'm the founder of [Rawkode Academy](http://rawkode.dev). it's our mission to help Senior Engineers excel at Cloud Native, Rust, Web Assembly and everything adjacent.

#### **Q: If you could give a pitch for Rawkode Academy, what would it be?**

**David:** Making the complex a little bit easier. We're in a world where we're all signed up to a race we didn't sign up for. Technology moves at a very fast pace, and I want to make that as easy as I can for everyone else in this industry.

#### **Q: When you first came across Golem and considered it for Rawkode Academy, what kind of solutions were you looking for?**

**David:** When I found Golem, I was looking to merge more Web Assembly into my existing workflows. I am very much intrigued by this current migration or adoption of durable execution, because I believe it changes the programming paradigm and makes programming simpler, which is obviously one of the key things I'm always looking for.

Tying that together with the performance benefits and sandboxing benefits of Web Assembly, I found Golem, and it's been fun ever since.

#### Q: What made you decide to prototype part of your backend on Golem specifically?

**David:** We decided to prototype on Golem, because I think it's the only thing in this space. It's truly unique right now. I haven't seen any other implementation of Web Assembly mixed with durable execution, that provides an interface to write a code in services.

It just worked the way I wanted it to, and Golem kind of ticked a few of those boxes. That's not to say there's lots of other Web Assembly runtimes in this similar space trying to do something similar, but Golem was unique in that durable execution was a core tenet of the entire programming model.

#### Q: What's your take on durable computing?

**David:** So there's nothing really new about durable execution, except they with put a name on something that really didn't exist before, but we've all been doing durable execution for a long time, but we did it through different patterns. That could be that we use an event-driven system or we have an event broker. The durable execution we do service-to-service handoff via events.

The programming model is really difficult, because you need to understand how all the events, and how all the subscribers, how it all works across an entire huge architecture, and it's really difficult to just see what is the workflow from a user signing up to the user getting the pizza at their house, assuming you were doing some sort of takeaway delivery system.

Durable execution abstracts that away to the point where we no longer need to worry about the backend implementation and the event brokers and all of that. We just write our code and say 'stop here and wait for something to happen' and the platform behind it handles the rest. That level of simplicity is giving people better understanding of the software that they're writing is almost invaluable. Not almost. It *is* invaluable.

#### Q: What kind of dramatic changes would you see if we were to implement something like Golem widely?

**David:** I'm pretty keen to implement Golem widely, because there are dramatic changes. The most dramatic change is that of each individual developer working on each individual workflow when they have the ability to look at a workflow in a single file, whether that be 50 lines or 200 lines or 2,000 lines.

Everything that they need to know exists there in front of their own eyes and they don't need to understand the system as a whole, which could be vast and complicated. So, the most dramatic change I'm looking forward to as we roll this out is just people being able to open a single file, understand exactly what they're building, and not have to go hunting through the rest of the architecture to work out the events and how they're consumed.

#### Q: What advice would you give to other companies considering Golem for their backend development?

**David:** The best advice is always going to be just try it. The APIs are simple. Web Assembly brings a new developer experience that we haven't typically had before when writing software. If we take a look at the past, we're migrating to containers to try and give truly ubiquitous global, and universal environment to work on things, and that's not really the case. As developers on your machines have different architectures, the containers are different. Web Assembly doesn't suffer that problem, so just go and try it out I think people will be pleasantly surprised by the developer experience and the API that Golem offers.

#### Q: What is the top feature or improvement that you'd like to see in a future update of Golem?

**David:** Golem already delivers almost everything that I need from a programming and computer perspective, which is fantastic. I think where I'd like to see some improvements in the future is around the automation and deployment model. Currently, it's very imperative. It requires a lot of command line automation, and I'd like to see friendly wrappers that allow me to have a slightly more declarative deployment mechanism to Golem Cloud.

-

Watch the full interview [**here**](https://www.youtube.com/watch?v=EjOpZeW4zwk)

Join us in the Golem 1.0 official launch on August 23rd at 12pm EST streamed live on X and LinkedIn.

[**Follow on Twitter**](https://x.com/GolemCloud)

[**Follow on LinkedIn**](http://www.linkedin.com/company/golem-cloud)

Be notified on launch details [**here.**](https://mailchi.mp/f4a02afe4fb0/invinciblebackends)

Join the community and ask questions [**here.**](https://discord.gg/UjXeH8uG4x)

---
title: "Building a Chatbot with Golem's Durable Computing"
author: "Golem Cloud"
slug: "building-chatbot-with-golems-durable-computing-interview-with-ivan-lopatin"
originalUrl: "https://golem.cloud/post/building-chatbot-with-golems-durable-computing-interview-with-ivan-lopatin"
date: "2023-12-04"
# date sourced from site-deploy timestamp "Mon Dec 04 2023" embedded in first wayback snapshot of post (web.archive.org/web/20231207194219/https://www.golem.cloud/post/building-chatbot-with-golems-durable-computing-interview-with-ivan-lopatin)
---

In chatbot development, maintaining a conversation's state without sacrificing system reliability can complicate the process. Ivan Lopatin, the second-place winner at the [Golem Cloud Hackathon](/blog/golem-cloud-hackathon), brings a fresh perspective to this challenge with his project: Media Tracker, a Telegram Chatbot designed to keep track of users' media consumption.

In this interview, Ivan reflects on the idea of durable computing and talks about how it might reshape cloud computing in the next decade. He also describes his journey with Golem Cloud, discussing how durable computing eased the state preservation in chatbot conversations, a common challenge in his previous developments.

**_"Thanks to durable computing, the progress of the dialog in my bot is automatically saved, and I don't have to worry about explicitly saving this data to external storage in case of an update or bot failure. When I heard about Golem, I immediately thought - this is the solution to my problem!"_**

The Media Tracker stands out as a proof of concept with the potential to evolve into a library for developing invincible chatbots on Golem. Dive in to learn about Ivan's experience with building on Golem, and explore why the seamless integration of durable state in Golem Cloud feels almost like a touch of magic to him.

#### What made you interested in participating in the Golem Cloud hackathon?

_I work at Ziverge, but not on the Golem team. I am very curious about John's and Ziverge's new project and follow its updates with interest. From the very beginning, I wanted to develop a small, but full-fledged project on Golem to properly try it out. The hackathon was a great opportunity to do so._

_And attractive prizes, of course!_

#### When you hear "durable computing", what comes to your mind?

_As a child, I read a sci-fi novel in which a guild of engineers maintained complex machines. These machines were so reliable that they worked without failure for many generations. The maintenance procedures became rituals, and the engineers became priests of the mighty machines, having lost the knowledge of how they actually worked._

_I imagine reliable programs that work for years without the need for maintenance, which is a large part of what developers and DevOps engineers do._

#### Where did you get the idea for your hackathon project?

_I love Telegram and I love bots in this messenger. I maintain a library for the Telegram Bot API in Scala (Telegramium) and have written two bots myself - for creating collaborative tasks in the family and for tracking my paid subscriptions. I also like to watch TV shows and read books, but I often forget which episode I stopped at, or the name of an interesting book I read a few years ago. So I came up with the idea of creating a Telegram bot to track content consumption. I am concerned about privacy and security of my data, and my own bot will allow me to use my own data storage._

#### Why was your project a fit for durable computing?

_In chatbot development, I was always concerned about preserving the state of the dialog so that a user could come back after a few hours or days and pick up the conversation where they left off. Thanks to durable computing, the progress of the dialog in my bot is automatically saved, and I don't have to worry about explicitly saving this data to external storage in case of an update or bot failure. When I first heard about Golem, I immediately thought - this is it, the solution to my problem. If you represent a dialog with a user as a separate worker, Golem itself will take care of preserving its local state._

#### How do you think durable computing will change cloud computing over the next 10 years?

_I believe, first and foremost, it will increase the efficiency and reliability of cloud solutions, which is always welcome. We will also see the spread and development of self-healing systems, edge computing, and decentralized cloud services. Certainly, the adoption of_ [_serverless computing_](/blog/serverless-with-golem-cloud) _will grow._

#### What was the best part of building a project on Golem Cloud?

_I like that using one of the main advantages of Golem Cloud - durable state - is almost free for developers, in the sense that there is no need to study any libraries or SDKs; you can write code in any supported language and the state is saved automatically. It feels like magic!_

_This was my first project in the Rust language, and during the Hackathon, I was able to focus on writing business logic in this wonderful language. Interacting with Golem, storing local state - everything is done using the language tools or HTTP libraries, and that's very cool._

#### Golem Cloud is very early, in developer preview. What sorts of features and improvements would you like to see first?

_Right now I would like to have the ability to interact with databases, even though Golem reduces the need to use them in many cases. A way to communicate with common databases using existing libraries would be useful in the development of many applications._

_The developers have already promised the ability to migrate Golem workers to a new version of the code; I am really looking forward to this, it will be an extremely useful feature._

_I would also like to see a strategy for integrating solutions on Golem with existing services and cloud infrastructures._

#### What types of resources do you recommend people study if they are interested in durable computing?

_From a broad theoretical perspective, one can start studying with books and research articles that focus on distributed systems, fault tolerance, resilience and reliability in computing. My favorite book in this area is the well-known Designing Data-Intensive Applications by Martin Kleppmann._

_On a more practical level, it is worth exploring the Golem Cloud blog, which talks about the concept of durable computing. I also found the materials and articles on durable execution from Temporal.io and durable functions from Microsoft Azure interesting, they helped me to understand the concepts and challenges of this area of computing._

#### Want to learn more? Here are some durable computing must-reads from our blog:

- [The Emerging Landscape of Durable Computing](/blog/the-emerging-landscape-of-durable-computing)
- [What Is Durable Computing?](/blog/what-is-durable-computing)
- [Exploring Serverless Architecture with Golem Cloud](/blog/serverless-with-golem-cloud)

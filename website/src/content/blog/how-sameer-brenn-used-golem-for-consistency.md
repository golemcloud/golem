---
title: "How Sameer Brenn Used Golem for Consistency"
date: "2024-08-15"
# date sourced from site-deploy timestamp "Thu Aug 15 2024" embedded in first wayback snapshot of post (web.archive.org/web/20240815*/https://www.golem.cloud/post/how-sameer-brenn-used-golem-for-consistency)
author: "Golem Cloud"
tags: ["Industry Articles"]
slug: "how-sameer-brenn-used-golem-for-consistency"
originalUrl: "https://golem.cloud/post/how-sameer-brenn-used-golem-for-consistency"
---

We had an amazing opportunity to interview Sameer Brenn, who experimented with Golem earlier this year.

<iframe allowfullscreen="true" frameborder="0" scrolling="no" src="https://www.youtube.com/embed/QZ1ZZ8UDzLk"></iframe>

#### Q: **What's your name and what do you work in?**

**Sameer:** I'm Sameer Brenn. I'm a software engineer with a background in finance and cryptocurrency.

#### **Q: What was your experience running into Golem?**

**Sameer:** I was in between jobs this year, and I decided to do side projects and experiment. I had heard about Golem, due to my knowledge of ZIO and Ziverge. I had built centralized exchanges in the past, and it was very challenging to build a highly scalable system that was reliable and consistent and provided the very significant consistency guarantees that you need for an exchange dealing with real money.

I heard about Golem, and it seemed like it had a really good potential for giving us the ability to build a highly consistent and scalable exchange in a much simpler way without all the traditional complex patterns of distributed computing

#### **Q: What are some things you wanted a solution like Golem to have?**

**Sameer:** What I was looking for is solving the problem of clear consistency in an exchange, particularly when you're doing your own settlement, because you need to make sure that when you have a customer in an exchange, and they're doing the trade, that they actually have the funds to do it. In a traditional, eventual, consistent distributed computing context, if you build things like we did at Twitter (*Sameer worked at Twitter a few years ago*), it was okay to have eventual consistency, because if a Tweet shows up 5 seconds late, it doesn't really matter that much. However, from a financial perspective, if you do a trade and you find out 5 seconds later that you didn't have the money to do the trade, then that's really bad. You need to have systems in place that guarantee that, but distributed systems make it really hard to do that.

I've been experimenting and trying to figure out ways to do that effectively, and that's when I turned to Golem. The other piece, of course, is scalability, because you could build a monolith that was completely consistent, but then you wouldn't be able to scale it to millions of users and many transactions per second. So, Golem seemed super appealing because it would allow us to provide that consistency that we needed to ensure that everyone's trades were reliable while still having scalability by having separate workers handling different things and the durability of things going down and coming back up without worrying about inconsistencies of system restarts.

#### **Q: What is your take on durable computing?**

**Sameer:** To be honest, I don't have a huge background in durable computing. I just spent a few months working on Golem, and I did a little bit with DBOS at the LambdaConf Hackathon a couple of months back.

It is certainly extremely appealing at the moment, in terms of the potential for really revolutionizing the world of computing and building strong systems to solve business problems. It's also a very early stage. I feel like a lot of the products out there are trying to figure out how it should work, what are the best ways to build it, because the DBOS, for example, is very different from the Golem approach. I'm sure they all have their benefits and their drawbacks. It's a very exciting area to be learning about and for people to be experimenting with.

#### **Q: What do you think the landscape would look like if all engineers were to adopt durable computing?**

**Sameer:** It would change things dramatically, because a lot of patterns are built around the traditional distributing computing models. Of course, it doesn't solve everything, so one of the things we have to figure out is what does it solve and what it doesn't, to know what needs to stay in the traditional, distributed systems world.

I think that, if we can move a lot of stuff over to durable computing, it will really accelerate the development of applications and really improve people's productivity and their ability to deliver value.

#### **Q: If someone were to consider Golem for their backend development or in general, what type of advice would you give them?**

**Sameer:** Because it's so different, it's good to experiment first for a while rather than just jumping in one hundred percent and moving your system over. From my experience, I was used to doing things a certain way, so when using Golem, it did take some learning to realize what are the proper patterns to use in this new environment.

Building a few prototypes at first would probably be a good approach because then you'll get a better understanding of the new patterns. Then, when you start building an application that you're hoping will be your production/service, you'll be able to do it right.

#### **Q: How was your experience working with the team?**

**Sameer:** I worked pretty extensively with the team when I was working with Golem pre-1.0. There were lots of pieces that the team had planned on working on or hadn't completed yet, and I was running into these difficulties because there were still some features missing, and then I would talk to the team, and they would plan to build it. They would prioritize the development, in part because I was asking for it. They were very receptive. Also, from a feedback perspective, I'm not 100% certain, but I believe that certain things I was asking that they hadn't contemplated, so it was very rewarding to feel that my feedback was incorporated into the road map.

-

Watch the full interview [here](https://www.youtube.com/watch?v=QZ1ZZ8UDzLk)

Join us in the Golem 1.0 official launch on August 23rd at 12pm EST streamed live on X and LinkedIn.

[**Follow on Twitter**](https://x.com/GolemCloud)

[**Follow on LinkedIn**](http://www.linkedin.com/company/golem-cloud)

Be notified on launch details [**here.**](https://mailchi.mp/f4a02afe4fb0/invinciblebackends)

Join the community and ask questions [**here.**](https://discord.gg/UjXeH8uG4x)

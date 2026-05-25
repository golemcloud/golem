---
title: "Golem 1.0 Hackathon 3rd Place Winner: Seeta Ramayya"
author: "Golem Cloud"
slug: "golem-1-0-hackathon-3rd-place-winner-seeta-ramayya"
originalUrl: "https://golem.cloud/post/golem-1-0-hackathon-3rd-place-winner-seeta-ramayya"
date: "2024-10-04"
# date sourced from site-deploy timestamp "Fri Oct 04 2024" embedded in first wayback snapshot of post (web.archive.org/web/20241007160639/https://www.golem.cloud/post/golem-1-0-hackathon-3rd-place-winner-seeta-ramayya)
---

We had the opportunity to interview the third-place winner of the Golem 1.0 Hackathon: Seeta Ramayya!

<figure>
  <iframe src="https://www.youtube.com/embed/ikL22vgDtfk" frameborder="0" allowfullscreen></iframe>
</figure>

**Do you mind sharing your name and where you're joining us from?**

**Seeta:** I'm Seeta, and right now I'm in India. Before the Golem Hackathon, I was in The Netherlands for 14 years, but I've recently moved back to India.

**If you could describe Golem in your own words, how would you describe it?**

**Seeta:** I may be wrong, but in my own words, this is how I saw it on that day.

I'm working on Akka actor systems in Scala and Scala-related projects, and I saw how Golem is like an Akka actor system. Akka actor systems come with each and every worker node as an actor, and the fleet act is where we define the APIs. Then, automatically, whenever there's a request that comes to it, the fleet actor forwards the correct actor, and that is how the API forwards to the correct worker node.

Worker-to-worker communication is essentially actor-to-actor communication. The worker nodes persist data, which sounds very much like how an actor system works with persistence.

We're talking about the scaling out scenarios where multiple actors will be created, not like you are hitting the limits of the boundaries of the particular machine, then you need actor clustering, so that multiple nodes can be joined.

**Based on your experience with Golem, what would you say is one of the uses that you can see Golem working for?**

**Seeta:** Yes, I may go back to my previous answer a bit. Wherever we use Akka actor systems and Akka actors, that's where we use Golem. Of course, an Akka actor system requires creation of actors, and all the code we need to write as developers, but at least it sounds like an establishment of the configuration, where you just need to write the business logic. Wherever we use the actors, that's where we can use Golem.

**What made you register for the Golem 1.0 hackathon?**

**Seeta:** John (De Goes) is actually a pretty well-known person in the Scala world. He's written multiple libraries, ZIO being one of them. Because I follow him on LinkedIn, I saw his post on the Golem Hackathon. I originally thought it was Scala-related, so I jumped in curious to know more. I've always been a Curious George since my childhood, so I wanted to see what it was.

That's how I ended up in the Hackathon, but then immediately, like 10 minutes in, I understood it wasn't related to Scala, and I thought about giving up. And although I wanted to give up initially, I figured I should have the same interest I had when I learned Scala to something new like Golem.

Then, as I progressed in the hackathon, it gave me small challenges while I was working on it. The challenges made me excited to continue. As a developer, I love challenging work. Once you get to solve those challenging elements, you feel very happy.

I ended up in this hackathon, without knowing anything and just pure influence of maybe John, but even after considering stepping out of the hackathon because it wasn't related to Scala, I was still interested in the challenges and fun of those two days.

**What would you say is your favorite aspect of developing on Golem?**

**Seeta:** Starting my career, I worked in Java for 4 to 5 years. When I moved to Scala, I felt as though I didn't have to write a lot of boilerplate code. It felt very concise and clean. Then, when I came across Golem, I felt a similar feeling when I realized I didn't have to write actors. It felt as though everything was given to me. You just need to write pure business logic.

Another thing I enjoyed is the APIs. If you configure and everything is done, and you've defined the API, then everything comes for free. I don't need to write anything.

Even though I didn't get to play a lot, I still loved to see how worker-to-worker communication works. In AWS, everyone says you should not leave backbone networks when applicable (cost effective, security and performance). So, that's how I tried to solve it, but that's not really efficient. I should use direct worker-to-worker communication because it is written within the boundaries of the application.

Because this was my first time playing around with Golem, I'm sure there will be other features that excites me more as well

**What other pieces of feedback would you give Golem on how it could improve?**

**Seeta:** Scala support! I would really like Scala support. Besides that, debugging was quite painful that needs more attention in the future. If something is going wrong, I want to know how to debug it. Even though the documentation is there, perhaps it would be good to tailor it some more to be able to find one specific thing. Maybe through searching or search boxes that could be helpful. Even leveraging tools like Chat GPT, it would still need to process all the documentation to be able to give you a tailored answer.

I must admit, because I'm a developer, I can be lazy when it comes to finding things, so I like automating my process so that I don't have to repeat myself. Also, a Chat GPT integration may be helpful.

For developers who are considering joining a future Golem hackathon, do you have any advice for them?

Just keep in mind that after you solve the challenge, you will find a lot of pleasure for the work you've done. Try to have a 'let it go' attitude and keep advancing while you can. You may be successful, and you may not. Either way, it's a learning opportunity. To be honest, I had no idea I was going to secure third place. I just enjoyed the time and learning throughout.

I also treated the hackathon days like a work day, and took the sleep I needed! Don't stress too much about finding certain results.

**Are you thinking about potentially competing in a future hackathon?**

**Seeta:** Of course, yes! I would definitely join. Maybe next time I'll play more with Scala and Golem.

-

For more information about the Durable Computing World conference, visit the DCW website [**here.**](https://www.durablecomputingworld.com/)

Get tickets for Durable Computing World [**here.**](https://www.eventbrite.com/e/durable-computing-world-tickets-940857188177?aff=oddtdtcreator)

Follow @GolemCloud on X for more.

Join our community[** here.**](https://discord.gg/UjXeH8uG4x)

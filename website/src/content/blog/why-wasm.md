---
title: "Why WASM?"
date: "2023-12-04"
# date sourced from site-deploy timestamp "Mon Dec 04 2023" at first wayback blog snapshot containing this post (web.archive.org/web/20231207190729/https://www.golem.cloud/blog?6732cdc5_page=2); post was absent from the 20230811 blog snapshot
author: "John A. De Goes"
slug: "why-wasm"
originalUrl: "https://golem.cloud/post/why-wasm"
---

Golem Cloud is a serverless cloud computing platform that hosts only applications that are compiled to WebAssembly, or which can be interpreted by an interpreter that is itself compiled to WebAssembly.

This is a stark difference to most cloud computing platforms, such as AWS, which let you deploy native code applications, JVM applications, and CLR applications, so long as you have the appropriate software.

What's the deal with WebAssembly, and why did Golem Cloud choose to support this technology, rather than taking a more traditional approach?

In this post, I will introduce WebAssembly for those who are new to the space, discuss why it matters, and talk about the reason Golem Cloud is supporting this technology.

### Origin

WebAssembly, or WASM, for short, was the successor to asm.js, a specification that was designed to allow compiled languages like C to target the browser (why should JavaScript have all the fun?).

Like asm.js before it, WebAssembly's goal was to become the "assembly language" for the modern web, allowing all programming languages to freely target browsers, while achieving very high performance compared to "transpiling" languages to JavaScript.

Making its debut in March 2017, WebAssembly enjoyed broad browser support, and as of today, 96% of all installed browsers support WASM. For the latest browser versions, this figure reaches 100%.

Although the specification was born in the browser, WebAssembly is not really a browser technology. It's much deeper than that.

### Specification

WebAssembly is a formal specification for portable machine code, similar to the Java Virtual Machine or CLR. Unlike x86 or ARM machine code, WASM cannot be executed directly by existing CPUs.

To be executed, a WASM program is either compiled into a platform (for example, Windows running on x86), or interpreted by another program that can run on such a platform.

The software that enables WebAssembly programs to actually be executed on a platform is referred to as a *WebAssembly runtime*.

The original WebAssembly runtime was, of course, the browser: browser engines added the ability to execute WebAssembly programs from JavaScript.

These days, however, WASM runtimes go way beyond browsers.

### Beyond Front-end

An increasing number of cloud providers, including Cloudflare, Fastly, Fermyon, and many others, now offer *server-side WASM*.

How did a technology intended to bring server-side languages like C to the browser end up back on the server-side??

There are a few key innovations driving this shift:

- **Edge Computing**. Edge computing is heterogeneous. WASM programs are portable and can be executed across virtually any platform, making WASM an ideal choice for edge platforms.
- **Performance & Latency**. Unlike other portable code formats like JVM, WASM runtimes are known for being able to execute programs with extremely low latency and near bare-metal performance.
- **Secure & Sandboxed**. To secure machine code, you typically have to virtualize the operating system. But WASM programs can only interact with the outside world through their host runtime. This means that they can be secured and sandboxed inside a single process, or even inside another WASM program.
- **Simple Deployment**. Due both to portability and security, together with sandboxing, WASM programs don't need to be containerized and virtualized, which can simplify architecture and potentially improve performance.

WASM gives us more than just a portable code format: it gives us a way to distribute high-performance, low-latency, secure, sandboxed code that can run on any machine, regardless of architecture, without the need for containerization or OS virtualization.

It's almost like a "virtual machine" designed for the cloud era: it has all the properties we want as a general-purpose format for transporting and deploying our programs into the cloud.

More interesting still, due to the unique design of WASM, in which a program can only access capabilities provided by its host, WASM is opening up new frontiers in cloud computing.

### Beyond Ordinary Backend

WASM is a gift to creators of cloud tooling. Not only can tooling inspect the structure of WASM programs and modify them, but more importantly, WASM programs interact with the outside world **only** through capabilities provided by their host environment.

By way of example, if you call `System.currentTimeMillis()` in your Java program, which is then compiled to WASM, then this ultimately gets translated into a call to a low-level WASI function that the WASM runtime provides to your WASM program.

Because the host provides *all* such functions, it means if you want to create powerful WASM tooling, you can do so by making your own host, and implementing your own low-level WASI functions, or by wrapping standard implementations, adding or modifying the behavior of any WASM programs that are executed by your host.

This allows some pretty interesting features, such as the following:

- **Time Traveling**. Custom hosts could record all the calls that your program makes to the outside world, and let you go back in time to understand how your program got into a failed state.
- **Logging**. Custom hosts could automatically log all interaction with the outside world so you don't have to write logging statements manually (at least, for most of your program).
- **Profiling**. Custom hosts could automatically profile all interaction with the outside world (and your program, too, by profiling the remainder) to show you bottlenecks in your application's performance.
- **Virtualization**. Custom hosts could make your program think it's interacting with local file system, when in fact it's interacting with shared networked file system.
- **Configuration**. Custom hosts could make your program think it's pulling system environment variables, when in fact it's reading configuration values from a config server or secrets server.
- **Embedding**. Custom hosts can embed your program as plug-ins, allowing you to script functionality of other applications in a highly secure way, without having to use a proprietary scripting language.

In essence, WASM makes it so that the ordinary-looking program that you write in whatever programming language you want (so long as it targets WASM) can behave in very special ways thanks to the unique constraints on WASM programs.

This is why Golem Cloud supports WASM.

### Golem Cloud: Powered by WASM

Golem Cloud is powered by a customized WASM runtime, which provides a custom host environment, with highly specialized implementations of all WASI functions that (ultimately) let your program interact with the outside world.

This lets Golem Cloud see all the interactions that your program makes with the outside world. Observing these interactions, together with having direct access to the memory of your program, allows Golem Cloud to continuously save tiny incremental changes as your program executes, ensuring that in the event of failure, your worker can resume exactly where it left off.

WASM gives Golem Cloud the capability to make programs in any programming language invincible--a feat that would be completely impossible with machine code, due to its highly unconstrained nature.

These benefits provide a compelling package for modern cloud developers, although the WASM community as a whole still has more work to do to reap the full benefits of this technology.

### WASM Improvements

The main drawback that WASM has right now is simply that it is an early-stage technology, without rich tooling and language support. Because of its infancy, not all programming languages target WASM.

Indeed, the low-level WASI interfaces needed to have programs compile to portable, standards-compliant WASM are not yet finalized, which means that supporting WASM is a bit of a moving target.

In addition, even for programming languages that do have a pathway to WASM, there are often rough edges. In a perfect world, you would simply add a `--wasm-wasi` flag to your compiler. And while we are close for some languages like Rust, the community still has more work to do before every language has this same experience.

### Summary

Golem Cloud is powered by WASM, a specification for a portable code format that is truly designed for the cloud era.

WASM is suitable for high-performance, low-latency applications, with portability across different architectures (ideal for edge computing), and has a highly secure, sandboxed design that improves performance and eliminates the need for containerization.

More than that, however, WASM has a compositional capability model that allows WASM programs to be run in special ways that gives you many capabilities. Golem Cloud takes advantage of this unique design to execute WASM programs invincibly, through a customized runtime that has access to your program's memory and interactions.

WASM is still in its early days, but given how it's simplifying deployment and opening up new frontiers in cloud computing, this technology is here to stay, and I look forward to the day when all mainstream programming languages have robust support for targeting WASM.

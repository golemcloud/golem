// About-page copy — single source of truth for /about.
//
// Body strings are raw HTML so inline tags (<strong>, <a>, <em>) survive
// the trip from data to template. Page renders headings declaratively and
// pipes each body into a Fragment with `set:html`.

export const meta = {
  description:
    "Founded in 2023, Golem Cloud builds the durable agent runtime — an open-source platform for stateful, fault-tolerant AI agents and workflows.",
};

export const hero = {
  eyebrow: "About",
  title: "Golem Cloud",
  lead: "Golem makes durable agents the default. State survives crashes. Tools execute exactly once. Policies are runtime guarantees, not application code.",
};

export interface Founder {
  name: string;
  bioHtml: string;
}

export interface AboutSection {
  heading: string;
  bodyHtml?: string;
  founders?: Founder[];
}

export const sections: AboutSection[] = [
  {
    heading: "Our story",
    bodyHtml: `<p>
      Golem began in March 2023, when <strong>John A. De Goes</strong> — mathematician, open-source advocate,
      and creator of <a href="https://zio.dev" target="_blank" rel="noopener">ZIO</a> — joined forces with
      longtime OSS contributor <strong>Daniel Vigovszky</strong> and a small-hand picked team. The first 
      prototype was hacked together in Inverness, Scotland, proving that transparent durable execution of
      WASM was possible — extending ZIO Flow's resilient-workflow ideas beyond the JVM.
    </p>

    <p>
      The first developer preview release of Golem followed later that year, on August 1, 2023. In 2024, 
      Golem Cloud Inc. was officially incorporated, and the source code to Golem went public, leading 
      to the first stable (if still primitive) release of Golem on August 23, 2024.
    </p>

    <p>
      As the Golem team tirelessly worked on improving usability, John recognized that the key strengths of 
      Golem &mdash; transparent durable execution, entity-orientation, formally-verified and cheap 
      sandboxing &mdash; all made Golem an incredibly compelling package to developers building agentic
      applications. So in May 2025, Golem began specializing for AI applications, leaving the broader 
      durable execution market to well-established and mature solutions like Temporal.
    </p>

    <p>
      Today, the Golem runtime is open-source under BUSL-1.1, transitioning to Apache 2. The Cloud service 
      remains in Developer
      Preview; paid general availability with formal SLAs and data-retention guarantees is planned for
      Q3 2026.
    </p>`,
  },
  {
    heading: "Founders",
    founders: [
      {
        name: "John A. De Goes",
        bioHtml: `<p>
          CEO and co-founder. Creator of <a href="https://zio.dev" target="_blank" rel="noopener">ZIO</a>,
          the open-source effect system that's been running in production at companies across fintech, ad
          tech, and AI infrastructure for the better part of a decade. Mathematician, open-source
          advocate, and longtime distributed-systems builder.
        </p>`,
      },
      {
        name: "Daniel Vigovszky",
        bioHtml: `<p>
          Co-founder. Veteran open-source contributor with deep roots in functional programming, runtime
          design, and WebAssembly tooling.
        </p>`,
      },
    ],
  },
  {
    heading: "The team",
    bodyHtml: `<p>
      We're a small, globally distributed team building Golem in the open. The runtime is written
      primarily in Rust; the SDKs span TypeScript, Rust, Scala, and MoonBit; and contributions come
      from developers across more than a dozen countries through <a
        href="https://github.com/golemcloud/golem"
        target="_blank"
        rel="noopener">GitHub</a> and our <a href="https://discord.com/invite/UjXeH8uG4x" target="_blank" rel="noopener">Discord</a>.
    </p>

    <p>
      Want to get involved? The fastest paths in are contributing on GitHub or joining the
      conversation on Discord.
    </p>`,
  },
];

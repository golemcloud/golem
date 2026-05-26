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
      longtime OSS contributors <strong>Daniel Vigovszky</strong> and <strong>Afsal Thaj</strong>. The
      first prototype was hacked together in Inverness, Scotland, proving that durable execution of
      WebAssembly components was possible — extending ZIO Flow's resilient-workflow ideas beyond the
      JVM.
    </p>

    <p>
      The open-source Golem runtime followed shortly after, with development happening publicly on
      GitHub. Golem Cloud, Inc. was incorporated in 2024, and the managed Cloud service launched into
      Developer Preview the same year.
    </p>

    <p>
      Today, the Golem runtime is open-source under BUSL-1.1. The Cloud service remains in Developer
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
      {
        name: "Afsal Thaj",
        bioHtml: `<p>
          Co-founder. Longtime OSS contributor across the ZIO ecosystem, with a focus on production
          reliability and distributed-system semantics.
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

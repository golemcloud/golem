// Careers-page copy — single source of truth for /careers.
//
// Body strings are raw HTML so inline tags survive the trip from data to
// template. The page renders section headings declaratively and pipes
// each body into a Fragment with `set:html`.

export const meta = {
  description:
    "Golem Cloud careers — how we work, our operating principles, and how to get involved when no roles are open.",
};

export const hero = {
  eyebrow: "Careers",
  title: "Building Golem in the open.",
  lead: "We're a small, globally distributed team building the durable agent runtime. We're not hiring right now — here's how we work, and the honest best path onto our radar when we are.",
};

export interface CareersSection {
  heading: string;
  bodyHtml: string;
}

export const sections: CareersSection[] = [
  {
    heading: "How we work",
    bodyHtml: `<p>
      We're a small, async team distributed across more than a dozen countries. Most communication
      happens in pull requests, GitHub issues, and our <a
        href="https://discord.com/invite/UjXeH8uG4x"
        target="_blank"
        rel="noopener">Discord</a>. The runtime is in Rust; the SDKs span TypeScript, Rust, Scala, and MoonBit. We ship in the
      open — every meaningful decision shows up in a commit, an issue, or an RFC.
    </p>`,
  },
  {
    heading: "Operating principles",
    bodyHtml: `<ul>
      <li>
        <strong>Open by default.</strong> Designs land as issues, RFCs, or PRs before they land as code.
        If something is worth doing, it's worth doing where others can read it.
      </li>
      <li>
        <strong>Correctness over haste.</strong> We're building infrastructure people will trust with state,
        money, and irreversible actions. We'd rather ship the right thing late than the wrong thing on time.
      </li>
      <li>
        <strong>Async and autonomous.</strong> Across many time zones, async is the default. We optimize
        for written artifacts that compound over time, not meetings that don't.
      </li>
      <li>
        <strong>Earn trust by shipping.</strong> The fastest path into the team is real contributions —
        code, ideas, bug reports, docs. We hire from the community first whenever we can.
      </li>
    </ul>`,
  },
  {
    heading: "No current openings",
    bodyHtml: `<p>
      We're not hiring right now. The honest best way to get on our radar when we are is to <a
        href="https://github.com/golemcloud/golem"
        target="_blank"
        rel="noopener">contribute to our open-source projects on GitHub</a> and show up in our <a
        href="https://discord.com/invite/UjXeH8uG4x"
        target="_blank"
        rel="noopener">Discord</a> community. We've hired multiple team members from the OSS community already, and it remains our
      preferred path.
    </p>

    <p>
      If you'd like to be considered when we open roles, the best move is to start contributing — we
      read every PR.
    </p>`,
  },
];

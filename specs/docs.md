# Documentation

- Status: **implemented**
- Scope: the public docs under [`docs/`](../docs), the repo `README.md`, the
  agent skill ([`skills/mira/SKILL.md`](../skills/mira/SKILL.md)), and how they
  relate to rustdoc and to `specs/`.

This is the design of record for Mira's public documentation: what lives where,
how pages are structured, and the conventions every page follows. It mirrors the
documentation practice in [everruns/everruns][everruns] so the two repos read as
one ecosystem — most notably, **diagrams are committed SVGs**, not raster images.

[everruns]: https://github.com/everruns/everruns

## 1. Where documentation lives

| Surface | Audience | Source of truth for | Format |
|---------|----------|---------------------|--------|
| `README.md` | first-time visitor | the pitch + 60-second quick start | Markdown |
| `docs/` | users | guides + the normative protocol reference | Markdown |
| rustdoc (on the crates) | API consumers | the exact types, traits, and signatures | `///` doc comments |
| `skills/mira/SKILL.md` | coding agents | the agent-facing overview + entry points | Markdown |
| `specs/` | maintainers | design of record (the *why* and the contract) | Markdown |
| `CHANGELOG.md` | upgraders | what changed, per release | Keep a Changelog |

One fact has **one home**. Guides link to the reference rather than restating it;
the README links into `docs/` rather than duplicating a guide; specs describe a
design once and docs describe its *use*. When a fact mirrors code (a protocol
version, a struct shape, a scorer name), the doc cites the code and is updated in
the same change — code is authoritative, docs track it.

## 2. Information architecture

`docs/README.md` is the single index and reading order. Every page in `docs/`
is reachable from it, grouped by intent:

1. **Start here** — `how-it-works.md`, `getting-started.md`.
2. **Authoring** — `authoring.md`, `scorers.md`, `metrics.md`, `subjects.md`.
3. **Extending** — `extensibility.md`.
4. **Reference** — `protocol.md`.

Adding a page means adding it to `docs/README.md` (and, if user-facing, the
README's Documentation list). Pages are topic-named, lower-kebab-case, one
concept each; split before a page sprawls past a single sitting.

## 3. Diagrams

The rule, carried over from everruns/everruns: **conceptual and architecture
diagrams are hand-authored SVG, committed to the repo** under `docs/assets/`.

- **SVG, not raster.** No PNG/JPG/screenshots for diagrams — SVG diffs, scales,
  and stays crisp on any display. Raster images are only for genuine screenshots
  (e.g. a rendered HTML report), never for boxes-and-arrows.
- **Self-contained & dependency-free.** Plain SVG with inline styling; no
  external fonts, scripts, or image refs. Use the system font stack
  (`-apple-system, Segoe UI, Helvetica, Arial, sans-serif`) so it renders
  identically everywhere, including on GitHub.
- **Responsive.** Set a `viewBox` (not fixed `width`/`height` on the root) and
  size at the embed site (`<img … width="640">`), so the diagram scales.
- **Accessible.** Every embed has descriptive `alt` text that states the
  relationship the diagram shows, not just its title.
- **Restrained, consistent palette.** Reuse the existing semantic colors —
  host (blue), study (green), subject (orange), connectors/labels (slate). A new
  diagram extends this palette rather than inventing one.
- **Named by subject.** `docs/assets/<topic>.svg`
  (e.g. `mira-overview.svg`); the canonical reference is
  [`docs/assets/mira-overview.svg`](../docs/assets/mira-overview.svg).
- **Registry-safe README embeds.** The repository `README.md` is rendered by
  off-repo surfaces such as crates.io, so SVG embeds there use absolute raw
  GitHub URLs instead of repo-relative `docs/assets/...` paths. Docs pages keep
  relative paths.

**Inline monospace sketches** (fenced ```` ```text ````) remain fine for small
wire/sequence diagrams whose value is alignment with surrounding JSON — e.g. the
framing and run-lifecycle sketches in `protocol.md`. Reach for an SVG when a
diagram is conceptual (how the pieces relate) rather than a literal byte/sequence
layout.

## 4. Writing conventions

- **Prose, not telegraph.** Public docs are full sentences in the present tense,
  addressing the reader as “you”. (The telegraphic style in `AGENTS.md` is for
  coding agents only.)
- **Lead with the model, then the API.** State what a thing *is* and why it is
  shaped that way before the method names.
- **Define a term once.** The canonical vocabulary — *host*, *study*, *subject*,
  *scorer*, *case*, *matrix*, *axis*, *transcript*, *N/A*, *infrastructure
  error* — is defined in `how-it-works.md` and used consistently everywhere.
- **Code examples are real.** Examples compile against the current API (prefer
  lifting from `examples/`); elide bodies with `/* … */`, never with stale
  signatures. CI builds docs with `-D warnings`.
- **Relative links** between docs/specs (`scorers.md`, `../specs/docs.md`);
  absolute `https://` links only for external targets. Deep-link to a heading
  (`scorers.md#llm-as-judge`) when pointing at one section.
- **Tables for vocabularies** (scorers, metrics, capabilities), prose for
  concepts.

## 5. The agent skill

[`skills/mira/SKILL.md`](../skills/mira/SKILL.md) is the agent-facing surface: a
coding agent loads it to learn how to author and run Mira evals. It is the *one*
place that orients an agent and then hands off; it never restates a guide.

Because the skill is **portable** — copied into other repos and read outside this
checkout — its conventions differ from the in-repo docs:

- **Absolute links.** All references to `docs/`, `examples/`, `sdks/`, and
  `specs/` are full `https://github.com/everruns/mira/...` URLs (blob for files,
  tree for directories). Relative links would break once the skill is detached
  from this repo. (This is the deliberate exception to §4's relative-link rule.)
- **Progressive disclosure.** Three levels: the frontmatter `name`/`description`
  (when to invoke) → `SKILL.md` (the overview) → bundled files under
  `skills/mira/references/` (depth, read on demand). `SKILL.md` stays skimmable,
  one concept per section; copy-paste recipes and lookup tables move to
  `references/`. Today: `references/cookbook.md` (subject + testing recipes) and
  `references/scorers.md` (the scorer catalog).
- **References travel with the skill.** Bundled `references/` files use *relative*
  links (`references/cookbook.md`) and are self-contained, so they work offline
  and when the skill is copied out. They are agent-curated lookups/recipes — the
  canonical, normative prose still lives once in `docs/` and is linked, not
  duplicated wholesale.
- **Install the binary.** The skill steers agents to the prebuilt `mira` CLI
  (`brew install everruns/tap/mira` or a Release binary), with `cargo install`
  as the source-build fallback only — see [`release-process.md`](release-process.md).
- **Cross-language entry points.** Always link the SDKs and `protocol.md` so an
  agent working in another language finds the polyglot path (`--cmd`).
- **Basic examples + `mira help --full`.** Point at the offline `examples/` and
  tell the agent the CLI carries its own full help.
- **Installable.** [`skills.sh`](../skills.sh) (at the repo root) copies this
  directory into a Claude Code skills root — `--global` (`~/.claude/skills`) or
  `--local` (`./.claude/skills`, the default). It copies from a checkout when
  present, else fetches from GitHub raw, so `curl -fsSL .../skills.sh | sh` works
  with only the prebuilt `mira` binary. Each run is a clean replace, so it is also
  the upgrade path. The file list lives in `skills.sh` (the `FILES` var) — a file
  added under `skills/mira/` is added there in the same PR, or it won't install.

A change to the surfaces the skill summarises — install method, the scorer/CLI
vocabulary, the example set, or the docs/SDK layout — updates `SKILL.md` and the
relevant `references/` file in the same PR.

## 6. Keeping docs in sync

A change to behaviour updates its documentation in the same PR:

- **User-facing change** → the relevant `docs/` guide.
- **Wire-format change** → `docs/protocol.md`, which is **normative** and carries
  the protocol version. The headline version and the per-method shapes must match
  `protocol::PROTOCOL_VERSION` and the Rust types.
- **Design decision** → the relevant file in `specs/`.
- **Agent-facing change** (install, vocabulary, examples, docs/SDK layout) →
  `skills/mira/SKILL.md` and the relevant `references/` file (see §5).
- **Anything user-visible** → `CHANGELOG.md` under `## [Unreleased]`.

This mirrors the “Docs in sync” ground rule in `CONTRIBUTING.md`; this spec is
the detail behind it.

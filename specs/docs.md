# Documentation

- Status: **implemented**
- Scope: the public docs under [`docs/`](../docs), the repo `README.md`, and how
  they relate to rustdoc and to `specs/`.

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
  *scorer*, *cell*, *matrix*, *axis*, *transcript*, *N/A*, *infrastructure
  error* — is defined in `how-it-works.md` and used consistently everywhere.
- **Code examples are real.** Examples compile against the current API (prefer
  lifting from `examples/`); elide bodies with `/* … */`, never with stale
  signatures. CI builds docs with `-D warnings`.
- **Relative links** between docs/specs (`scorers.md`, `../specs/docs.md`);
  absolute `https://` links only for external targets. Deep-link to a heading
  (`scorers.md#llm-as-judge`) when pointing at one section.
- **Tables for vocabularies** (scorers, metrics, capabilities), prose for
  concepts.

## 5. Keeping docs in sync

A change to behaviour updates its documentation in the same PR:

- **User-facing change** → the relevant `docs/` guide.
- **Wire-format change** → `docs/protocol.md`, which is **normative** and carries
  the protocol version. The headline version and the per-method shapes must match
  `protocol::PROTOCOL_VERSION` and the Rust types.
- **Design decision** → the relevant file in `specs/`.
- **Anything user-visible** → `CHANGELOG.md` under `## [Unreleased]`.

This mirrors the “Docs in sync” ground rule in `CONTRIBUTING.md`; this spec is
the detail behind it.

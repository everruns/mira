# mira-judge

LLM-as-judge [`Scorer`](https://docs.rs/mira-eval)s for
[Mira](https://github.com/everruns/mira), backed by real providers.

Mira's core stays provider-agnostic and dependency-light (`mira-eval` carries no
HTTP stack). This crate is the integration layer: it wires an `LlmJudge` onto an
actual model endpoint and exposes it as an ordinary `Scorer`, so it composes with
the deterministic built-ins and combinators unchanged.

Three provider transports are supported:

- `LlmJudge::openai_completions` — OpenAI **Chat Completions** (`/v1/chat/completions`).
- `LlmJudge::openai_responses` — OpenAI **Responses** (`/v1/responses`).
- `LlmJudge::claude` — Anthropic **Messages** (`/v1/messages`).

A judge depends on a network call, so it *will* sometimes fail for reasons that
have nothing to do with the subject (no API key, rate limit, 5xx, timeout). In
those cases the scorer returns `Score::na` — neither pass nor fail — rather than
crashing the run or scoring a spurious `fail`. A run with no credentials
therefore stays green: every judge cell is simply N/A.

`Include` selects the surface graded: just the agent's final response, the
response plus its tool calls, or the full picture including operational metrics
(tokens, cost, latency). Pick the narrowest surface the rubric needs.

See the runnable [`examples/llm_judge`](https://github.com/everruns/mira/tree/main/examples/llm_judge)
for a full wiring alongside deterministic scorers. Licensed under MIT.

# mira-publish-everruns

Publish a saved [Mira](https://everruns.com) eval run to an
[everruns](https://everruns.com) instance, which hosts and visualizes eval
results it did **not** execute. This lets a study run against any provider or
CLI subject and then publish its results for hosted comparison — no need to run
inside everruns' session system.

```bash
# one-time: authenticate the everruns CLI (mira reuses its credentials)
everruns login

# run an eval and publish the saved run
mira run --publish everruns

# or publish a previously saved run by id
mira publish <run_id>
```

## Authentication

Credentials resolve the same way the everruns CLI resolves them, first match wins:

1. explicit `--api-key` / `--api-url` flags,
2. `EVERRUNS_API_KEY` / `EVERRUNS_API_URL` / `EVERRUNS_ORG_ID` environment variables,
3. the everruns credentials file (`~/.config/everruns/credentials.json`),
   selecting `--profile` or its `current_profile`.

The token is a personal access token (`evr_pat_…`) sent as a bearer token; the
target org is sent via `X-Org-Id` (a single-org user needs none).

## What gets published

One Mira run becomes one everruns *run group* (one EvalRun per Mira `eval`, all
sharing the Mira run id). Each `(sample, target)` result becomes one case
result carrying its scores, a normalized transcript, and an open-vocab metrics
bag (cost, cache/reasoning tokens, time-to-first-token, study metrics).
Publishing is idempotent on the run id: re-publishing replaces the prior run.

everruns trusts Mira's verdict — it stores and displays the scores, it does not
re-grade.

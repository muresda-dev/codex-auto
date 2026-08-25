# Codex Auto model router v3

The `Auto` model selection mode chooses a model and reasoning effort for each root user turn instead of treating `auto` as a model name.

## Routing tiers

- **Luna** — routine, low-risk, high-volume work where upgrading model quality has little expected value.
- **Terra** — the default engineering tier for analysis, debugging, implementation, and ambiguous work where the Luna → Terra gain is meaningful.
- **Sol** — complex architecture, migrations, high-risk changes, or tasks where the Terra → Sol gain justifies the extra cost and latency.

Reasoning effort is selected independently from the model. `max` is reserved for exceptional depth or critical high-reasoning work; ordinary complex architecture should normally use `high`.

## Policy inputs

Router v3 builds a task profile from the current user input and conversation state. It considers task kind, depth, ambiguity, risk, scope, structured/code content, non-text modalities, error/debug signals, previous-turn context, retries, and short follow-up/continuation prompts.

Long prompts do not automatically imply a stronger model. Short continuation prompts can inherit the previous route, while retry/failure signals can escalate it.

The router remains capability-aware: a preferred tier is only selected when the catalog model is selectable and compatible with the turn modalities and reasoning requirements. Catalog fallback is explicit in the route result.

## Pairwise gain and stability

The policy estimates the expected quality gain for:

1. Luna → Terra
2. Terra → Sol

Risk floors can force a minimum tier. Confidence and hysteresis reduce unnecessary model flapping near decision boundaries.

Role-owned and internal subagents are excluded from root Auto routing so explicit role model/reasoning settings are not overwritten.

## Explainability

The core route event and app-server notification include:

- selected model and reasoning effort
- route class
- confidence
- Luna → Terra and Terra → Sol gain scores
- contributing signals
- continuation inheritance flag
- retry escalation flag
- catalog fallback flag

In the TUI, run `/route` after an Auto-routed turn to inspect the latest routing decision. The status display continues to show the compact `Auto → <model> · <effort>` label.

## Validation

Router v3 includes a regression benchmark covering routine, analytical, complex, exceptional, continuation, retry, risk, and capability/fallback scenarios. The Windows self-hosted CI additionally runs formatting, router tests, affected-crate checks, release builds, and packages `Codex-Auto-Setup-x64.exe` with Inno Setup.

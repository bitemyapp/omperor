# Token usage and prompt caches

The Usage window shows **Input tokens**, **Output tokens**, **Cache reads**, and **Requests**, including recorded workers. Compact numbers have exact-count tooltips. A `+` on cache reads marks partial telemetry; unavailable cache counts display a dash, while explicitly reported zero remains zero.

The **Usage** window (the toolbar's chart button, or Session ▸ Usage…) has metric cards, an input-composition bar, owner/model tables, and separate **Overview**, **Requests**, and **Cache policy** pages. Reading it never invokes a model. Journal reads, worker discovery, aggregation, and the tables' preparation run on a worker; the window shows that it is reading until they arrive. It reads the session in view when it opens, and **Refresh** rereads the ledger. A generation on each read keeps a late result from replacing a newer one. `/usage` puts the same totals, with the accounts' limits, in the conversation as a card.

The ledger includes committed receipts from every branch of the selected journal and its recorded descendants. Stable receipt IDs prevent copied receipts from being counted twice. Root and worker totals are separate. Missing journals, unsupported legacy transcripts, malformed receipts, and analysis limits are visible. This is request consumption, not the number of unique tokens currently in context.

## Accounting contract

`input = uncached input + cache reads + cache writes`. These input classes are exclusive; one-hour writes are a subset of writes. Reasoning is already included in output. OpenAI's inclusive wire input is normalized by subtracting both cache reads and cache writes. Anthropic sends the classes separately.

Patch `0010-cache-usage-telemetry.patch` preserves receipt identity, timestamp, usage source, per-counter presence, one-hour writes, and catalog-cost provenance through the journal and RPC. The GUI also preserves these fields in its metrics log. A reported zero differs from an unavailable field. Older receipts cannot acquire missing provenance retroactively.

Catalog dollar estimates are neither bills nor subscription quota. They use the helper's catalog and micro-dollar rounding, and can include retry estimates without corresponding committed token detail. Failed or interrupted calls and auxiliary calls without receipts can consume additional tokens; the ledger explicitly states this coverage limit. Cache-read share with incomplete telemetry is labeled partial.

## Provider policy

The typed reference is [`src/cache_policy.rs`](../src/cache_policy.rs), verified on **2026-09-22**, with Anthropic's default and OpenAI's response retention fields rechecked on **2026-10-06**. Omperor uses a conservative **5-minute estimate for OpenAI API, Codex subscriptions, and Anthropic** when effective retention is unavailable. Anthropic's one-hour TTL requires an explicit request. These provider-wide fallbacks use `CacheTtl::Assumed`, distinct from confirmed request settings, and affect warnings rather than provider request parameters. Model-specific thresholds and prices remain unknown for unrecognized models. API price ratios are references relative to ordinary input, not claims about a subscription's quota deductions.

| Route | Retention reference | How Omperor treats it |
| --- | --- | --- |
| OpenAI API, documented GPT-5.6 and GPT-6 models | Minimum 30 minutes after eligible reads/writes; `prompt_cache_options.ttl` | A returned `30m` TTL drives a 30-minute timer; otherwise five minutes |
| Earlier OpenAI API models | In-memory typically 5–10 minutes, at most 1 hour; extended typically 30 minutes, at most 24 hours | Five-minute timer unless a minimum TTL is returned; legacy policy ranges remain reference information |
| OpenAI Codex subscription | No subscription TTL confirmed | Five-minute fallback unless the response explicitly reports a minimum TTL; API billing remains reference information |
| Anthropic | Five minutes by default when caching is enabled; one hour requires an explicit request | Five-minute estimate in the absence of returned TTL metadata; request preferences do not become provider-reported evidence |

The helper's default Anthropic `Auto` retention currently leaves the request unset and emits no explicit cache markers. Its `short`/`long` mappings are 5m/1h, but marker placement needs separate correction before those preferences should be promoted as a reliable caching switch. This change exposes the issue; it does not silently alter retention configuration. OpenAI preferences also do not always map to wire values, and Codex responses-lite strips `prompt_cache_retention`.

The public Responses API can return `prompt_cache_options.ttl` for GPT-5.6 and later as the minimum lifetime applied to cache breakpoints; the documented value is `30m`. Older `prompt_cache_retention` values describe a retention policy, not an exact expiry timestamp. Patch `0020-response-cache-timer.patch` normalizes a returned positive TTL into `prompt_cache_ttl_seconds` in the journal and `promptCacheTtlSeconds` in live RPC. It also preserves recognized legacy retention policies (`prompt_cache_retention` / `promptCacheRetention`). Unknown or malformed values leave the minimum TTL unavailable without failing inference. Both fields survive the secure-gate spool and receipt replay; neither is inferred from request settings or model defaults. The public API contract does not establish Codex subscription behavior. A cached-token count records a hit on that request and does not report remaining lifetime.

The timer starts at **client-side response completion**, recorded before OMP's
journal writes and message-end hooks as `response_completed_at_ms`
(`responseCompletedAtMs` in RPC). Older receipts use their recorded completion
time, or the journal entry timestamp, without subtracting request duration.
Every completed response refreshes the timer, including responses without cache
hits, writes, token counters, or a duration. Failed or interrupted streams that
produce no receipt do not restart it. Worker receipts do not refresh the parent
chat's timer. A later response with no TTL returns to the five-minute estimate;
it does not inherit an earlier response's reported TTL.

A reported minimum TTL controls the countdown independently of the model name.
A legacy `24h` maximum or `in_memory` policy does not establish a minimum, so
those still use the five-minute estimate. The Usage page displays the same
completion-based timer as a snapshot; Refresh updates it. The countdown is an
application convention, not an exact server expiry: providers measure retention
from cache operations, and eligible prefixes may be retained beyond the timer.

## Changes and warnings

Policy actions describe model/account namespace changes, request-level reasoning and speed settings, instructions, tool schemas/order, edited history, compaction, image presence, and new/forked journals. Changes affect a prefix from the first mismatch; earlier matching context may remain reusable. Compaction can lower total consumption despite fewer cache hits. A receipt does not identify why a miss occurred.

Model, account and reasoning changes, applicable speed changes, first Anthropic image attachment, and Codex new/forked sessions proceed without a cache confirmation dialog. When an existing conversation may have reusable context, its KV timer badge beside the attachment button gains a warning explanation. Hover for an explanation; click for an informational dialog. The warning belongs to the affected chat and clears after its next successful turn. Failed or interrupted turns retain it. Unchanged values and ordinary append-only messages do not add warnings. Image warnings are conservative on resumed sessions whose earlier image presence is not available to the composer.

The composer always shows a **KV cache timer**: `KV —` before the first completed
response, `KV ~05:00` counting down for the fallback, or (for example) `KV 30:00`
when the provider explicitly reports a 30-minute minimum TTL. Hover text explains
the evidence, the five-minute assumption when applicable, and the completion
anchor. The badge turns amber in the final minute and stops at zero. Zero means
the displayed window elapsed; it does not assert that a cache entry existed or
was evicted. Clicking opens the informational dialog; it never opens on its own.

Live receipts update the clock immediately, and background usage reads restore
it from the selected chat and model's history. Replaying the same receipt or
selecting a chat does not restart the clock. Account/model changes reset it and
reject stale reads; historical receipts do not identify the account that created
them. The idle timer checks only in-memory state once per second. Action warnings
share the badge and its hover explanation without hiding the countdown.

The isolated `--cache-warning-smoke` scenario starts five seconds before the final
minute, then reaches zero after another minute. Set
`OMPEROR_CACHE_PREVIEW_TTL_SECONDS=1800` to inspect a provider-reported TTL instead
of the fallback. These native previews use synthetic timing and no inference.

External edits to system instructions/tool configuration and helper-initiated compaction are explained in the policy reference; the GUI cannot intercept those external or autonomous actions. It does not keep caches alive with extra requests.

## Sources

- [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
- [OpenAI response object and applied cache options](https://developers.openai.com/api/reference/cli/resources/responses/methods/retrieve)
- [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [Anthropic tool use and caching](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-use-with-prompt-caching)

Offline checks cover provider policy, unknown routes, full model identifiers, TTL boundaries, scoped invalidation, token classes, missing versus zero, copied-receipt deduplication, worker discovery, journal/RPC compatibility, and custom binary usage round trips. Deterministic native previews cover fallback and provider-reported timers; real inference checks use the designated testing account.

Validated on 2026-10-06: 377 Omperor tests passed (two environment-dependent
tests ignored), plus 71 focused helper tests and the paired doctests. Native
previews verified estimated and reported countdowns, the final-minute color,
zero, click-only dialogs, and narrow composer layout. The signed release bundle
completed a live OpenAI (Codex) / `openai-codex/gpt-5.6-luna` response. It returned
`prompt_cache_retention: "24h"` with no minimum TTL; the GUI and metrics preserved
that distinction and used the five-minute estimate. The recorded stream-end
timestamp preceded receipt creation by 31 ms. Reopening the app and conversation
restored the remaining time and legacy policy without restarting the countdown.
The explicit-minimum-TTL path was validated with decoder fixtures and the native
preview; that live response did not supply one.

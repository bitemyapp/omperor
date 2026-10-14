# Model discovery

Omperor's model picker is populated by the OMP helper. The GUI does not own a
list of model versions. The bundled catalog supplies provider wire contracts,
known metadata, and an offline fallback; a model does not need to be in that
catalog to appear in the picker.

For Anthropic, authenticated discovery uses `GET /v1/models?limit=1000` and
follows `after_id` pagination. OAuth and API-key authentication use the existing
provider credential stack. The old Claude CLI bootstrap route is migrated at
the codec boundary until the upstream embedded catalog adopts the Models API.
Bootstrap's `additional_model_options` is not a complete model list. In
particular, it can return a client-update sentinel instead of a new model ID.

For OpenAI Codex, the existing authenticated model discovery route remains in
use. Both providers cache their successful listings in `models.db` for two
hours. Startup refreshes stale or pre-migration entries. **Refresh Models**
bypasses the TTL and reloads the helper's catalog, resuming recorded sessions.
An empty session that has never persisted is replaced by a fresh session.
The model-discovery TTL is unrelated to provider prompt/KV cache TTLs.

Successful Anthropic responses determine the visible model set. There is no
fixed subscription model allowlist. Newly discovered rows retain their wire
IDs, names, input/output limits, and reported capabilities. Compatible adaptive
reasoning policies are selected by the reported effort contract, not the model
version. Unknown contracts and prices remain unknown; the Models API does not
supply prices.

A failed refresh does not replace a successful cached listing or clear the
current selection. Complete failure is reported as an RPC error; partial
success includes per-provider warnings. Startup can use the last successful
listing after its TTL expires. Explicit account pins isolate cache namespaces;
a different account's listing is not substituted. When no prior Anthropic
listing exists, startup can expose the bundled chat models as a fallback.
These fallbacks are not new provider availability confirmations.

Discovery runs on the existing OMP runtime worker. The AppKit main thread does
not make network requests or wait for the helper. Fetching a model list does not
send a prompt or generate model tokens.

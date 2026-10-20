# Exclusive model selection

The desktop profile stores `selected_model`. Clicking a detected model selects it; Save persists
the selection and Connect starts the bridge with `BRIDGE_SELECTED_MODEL`. Changing the model server
clears the selection. An incomplete profile cannot connect. The selection is immutable while running.
Discovery never loads a model. Requests for the selected model can still trigger that runtime's
normal just-in-time loading; requests for other models never reach the runtime.

With `selected_model` configured, the bridge authenticates and rate-limits callers as usual, then:

- `GET /v1/models` returns only the selected id actually present in the latest status snapshot.
  Before the poller has observed it, the list is empty. No model is fabricated from configuration.
- `/status` and connector status announcements contain only the selected model.
- JSON `POST /v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, and `/v1/responses`
  require an exact `model` match. Missing, duplicate, malformed, or non-string model fields return
  400; another id returns 403. The body is never rewritten to disguise a different requested model.
- Other proxied paths, methods, query strings, and runtime-native management endpoints return 403.
  This prevents bypassing the selection through a model-loading or alternate inference endpoint.
  Bridge-owned health, status and node-observability routes retain their existing behavior/auth.
- Multi-upstream routes combine with selection only as a **pool of the selected model**: every route lists
  exactly the selected id as its only model and rewrites nothing (`upstream_model` unset). Startup rejects any
  other route, so no model rewrite or routing fallback escapes the selected-model boundary; the admitted request
  always names the selected id, so it always matches the pool and never falls back. `nexa-bridge pool add` writes
  such routes ([server-setup.md#pool](server-setup.md#pool)).

Unset selection retains the headless bridge's existing multi-model behavior. The desktop requires
a selection before connecting; it never chooses an arbitrary first model. Existing saved profiles
without a selection remain editable but cannot auto-connect until a model has been selected and saved.
The public tunnel frame/status schema is unchanged; this is a box-side admission policy, not a relay change.
Callers must use the selected id rather than a cloud alias. Selection is not a substitute for bearer auth.

The model list uses the same periodically refreshed snapshot as `/status`, so it has the existing
poll interval rather than promising instantaneous availability. Inference responses remain streamed
byte-for-byte; rejection happens before upstream routing or in-flight counters are touched.

## Your workspace's models

The workspace chooses the Nexa preset this connection serves on Agora's Nexa Bridge page (Settings › Nexa
Bridge); its model pickers then offer that preset through this connection, and a hop of its model settings may
also name it. Managed installations read them from `/api/v1/bridge/managed/device/guides` with the device proof
from the OS credential store: each preset's name, Nexa's certification label, the **exact model id** the
workspace sends, the context window to load, and Nexa's guide for each runtime (steps and flags for LM Studio,
Ollama, llama.cpp, vLLM, SGLang or another OpenAI-compatible server). The panel shows the guide of the detected
runtime, every runtime's before detection, and marks which preset the selected model serves. Nothing is applied
automatically: the person loads the model under that exact id with those settings, and what the model server
loads is their responsibility. A selected id no preset uses is allowed but flagged, since the workspace's requests
name the presets' ids and the bridge answers only the selected one.

Connect publishes the exact served id, the detected runtime and, on LM Studio, the context of the exact loaded
instance (not the model's advertised maximum) when it can be read; an unreadable context is logged and published
as absent, never a condition to connect. It sends `profile: null`, which clears a profile an earlier version
recorded. Agora never writes a preset from a computer's choice. Selection never grants tools or platform
authority, and a certified preset does not certify the user's weights, quantization, installation or location.

Settings files written by earlier versions keep loading: their `selected_profile` is ignored and dropped on the
next save.

## Verification

Pure policy tests cover exact matching, missing/duplicate fields, blocked endpoints and query strings.
Settings tests cover selection persistence and incomplete profiles. Live verification must additionally
check the selected list/status, successful selected-model inference, rejected other-model inference,
and blocked native model-loading paths against the real bridge and runtime.

Local gates:

```sh
cargo clippy -p nexa-llm-bridge -p nexa-desktop --all-targets --locked --offline -- -D warnings -W clippy::pedantic
cargo test -p nexa-llm-bridge -p nexa-desktop --locked --offline
./crates/desktop/sync-sidecars.sh
cargo build -p nexa-desktop --locked --offline
node --check crates/desktop/ui/app.js
```

Native acceptance exercised Detect → click loaded model → Selected → Save and verified the persisted
`selected_model`. A separate real bridge on an unused loopback port was started with an ephemeral API
key and `BRIDGE_SELECTED_MODEL`. Its `/v1/models` and `/status` returned only the selected id. Missing
or incorrect credentials returned 401; other-model inference returned 403 on all four admitted paths;
missing and duplicate model fields returned 400. Native model-loading paths and query-string variants
returned 403. A selected-model SSE completion contained final content, usage, and `[DONE]`. LM Studio's
native model loading states were identical before and after; the six other models stayed unloaded.
The temporary bridge was stopped and its generated key was never persisted. This is local live
acceptance, not a new relay deployment or a claim of cross-platform validation.

## Changelog

- 2026-09-06: Add explicit versioned Agora profile selection, fail-visible catalog refresh and
  loaded-instance context validation. Keep recipes and credentials outside the desktop projection.

- 2026-09-05: Add exclusive desktop model selection, persisted profiles, filtered discovery/status,
  and fail-closed request admission without changing tunnel framing or caller model ids.
- 2026-09-05: Resolve pedantic findings in the affected bridge gate without suppressions: explicit
  clone/Option handling, complete redacted config Debug, unit signal patterns, cast-free GPU sample
  averaging, and extracted upstream request construction retaining existing header/body semantics.

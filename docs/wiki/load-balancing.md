# Load balancing & affinity

Source: [`crates/bridge/src/app.rs`](../../crates/bridge/src/app.rs), function `select_candidates`.

Several `[[upstreams]]` entries serving the **same** `models` name form a **pool**. On a server linked with the
command line, `nexa-bridge pool add` writes them ([server-setup.md#pool](server-setup.md#pool)). That is how one
model scales across heterogeneous devices — a multi-GPU server, a workstation, a Mac — behind one
endpoint.

Selection produces an **ordered list of candidates**, not a single choice. The proxy walks it until
one answers, so every ordering decision below is a preference, and the full failover chain always
survives.

## The order

### 1. Context fit — the only hard exclusion

A device whose `max_body_bytes` cannot hold **this** request is dropped from the pool for **this**
request. A 32K-context machine pooled behind a 64K one must never be handed a prompt it would reject.

If *no* device fits — a prompt larger than every capped peer — the full matching set is used
best-effort. Refusing to route at all would be worse than trying.

### 2a. With `X-Affinity-Key` — rendezvous hashing

The pool is sorted by **rendezvous (highest-random-weight) score** over `(key, upstream url)`,
descending.

The same key therefore always leads with the same device. That is the point: consecutive rounds of
one agent turn land on a warm prefix cache instead of re-prefilling the conversation from scratch on
whichever device happened to be next. Measured on a two-device pool, bouncing between devices spent
**54% of total LLM time re-prefilling**.

Rendezvous hashing also gives a deterministic *tail*, so a sick primary degrades to one stable
secondary rather than scattering. And adding or removing a device only remaps the keys that hashed
to it, instead of reshuffling everything.

The header is consumed at the bridge and never forwarded upstream. A sensible key convention is one
value per logical conversation — e.g. `t<tenant>:a<agent>`.

**The hash is FNV-1a, implemented inline on purpose.** `std`'s hasher makes no cross-release
stability guarantee, and a silent remap on a Rust upgrade would cold every prefix cache in the fleet
at once. The implementation is pinned by test against the canonical FNV vectors.

### 2b. Without a key — least connections

The pool is rotated by a shared round-robin cursor, then **stably** sorted by in-flight count
ascending. So the idlest device leads, and round-robin breaks ties, which keeps equal-load peers
rotating instead of all piling onto whichever one sorts first.

### 3. Band demotion

Whichever order came out of step 2 then passes a **stable** sort into three bands:

| band | contents |
| --- | --- |
| 0 | available, under its concurrency cap |
| 1 | available, but **saturated** (`max_inflight` reached) |
| 2 | inside a **circuit breaker** cooldown |

Order within a band is preserved. With no `max_inflight` configured and no tripped breaker every
device is band 0 and the sort is a no-op — behaviour identical to not having the feature.

## Saturation escape

Affinity deliberately ignores load: the prefix-cache win dwarfs mild imbalance. But it cannot ignore
it *without limit*. A hot key pinned to one device would queue there while its peers idle.

`max_inflight` is the release valve. Set it to the device's real concurrency budget — vLLM's
`--max-num-seqs`, say. At or over that number the device drops to band 1 and an under-cap peer takes
the request, **including** for affinity-pinned keys. The moment it drains, the pin resumes. Both
states are deterministic, so this does not oscillate.

If every device is saturated the band sort is uniform and the original order stands.

## Circuit breaker

Per-request failover already handles a device that fails fast. The breaker exists for the case
failover cannot see: a **wedged server** — a dead engine behind a live HTTP front. It accepts the
connection and then never answers. Connect-level failover never triggers, so without cross-request
memory every affinity-routed request would queue serially into the stall, each one waiting out the
read timeout.

So outcomes are remembered per upstream:

- `breaker_failures` **consecutive** failures (connect error, read timeout, or 5xx) trip it.
- While tripped, the device sits in band 2 for `breaker_cooldown_secs` — demoted, never excluded.
- When the cooldown expires the next request probes it naturally (**half-open**). A success fully
  resets the state; a failure re-trips immediately, since the counter is still at threshold.
- Any success at any time resets the counter. An upstream with no observations is assumed healthy.

If the health mutex is poisoned, selection **fails open** — routing everywhere beats routing nowhere.

## Fallback

A model with no matching route, or a model-less or non-JSON body, yields the single default
`upstream_url` with no rewrites. This is the pure single-upstream path.

## What the pool reports {#reported}

The bridge's `/status` lists every model server of the pool as `{url, healthy, inflight?, max_inflight?,
circuit_open?}`: the last probe's answer, the requests in flight to it now, its configured ceiling, and whether the
circuit breaker sets it aside. The three load fields are optional and absent from bridges older than them. The
connector forwards the same rows up the tunnel, so Nexa Agora's Nexa Bridge page reads the pool of every computer of
a connection, and `nexa-bridge pool` prints one line per server (`answering · 3/16 in flight`, `circuit open`).
Above the pool, the relay serves a connection from every computer linked to it — rendezvous pick on
`X-Affinity-Key`, a dropped tunnel skipped as the request is sent — so two computers that each pool the same model
servers keep the model up when either machine goes down.

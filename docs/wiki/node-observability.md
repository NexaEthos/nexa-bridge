# Node observability — `/_node/*`

Source: [`crates/bridge/src/node.rs`](../../crates/bridge/src/node.rs).

The bridge is the only durable, authenticated process on the box, so it doubles as the box's node
exporter. Both endpoints ride the **same `api_key` gate as the serving path** and are reachable
through the tunnel, which means a control panel can show GPU load and logs for a machine it cannot
otherwise address.

## `GET /_node/stats`

Returns JSON: per-GPU state from `nvidia-smi` (index, name, utilisation, memory used/total,
temperature, power), 1/5/15-minute load averages, CPU count, total and available memory, plus a
`history` array.

**Missing sources degrade to `null` rather than failing.** A box with no NVIDIA GPU still reports CPU
and memory; a non-Linux host still reports what it can. A malformed `nvidia-smi` row is skipped, not
fatal.

`/proc/loadavg` and `/proc/meminfo` are Linux-only, which is why the test covering them is
`#[cfg(target_os = "linux")]` — on other dev hosts those fields are simply `null`.

The payload also carries `hostname` (the node's `/etc/hostname`, trimmed) and
`log_units` (the configured journald whitelist, verbatim). A console renders one log pane per
listed unit and labels every pane with the hostname — nothing about the node is hard-coded on
the console side; swapping a serving unit is a config edit on the node and nothing else.

## The history ring

A background task samples average GPU utilisation, total GPU memory used, and 1-minute load every
**30 seconds** into a ring of **240 points** — about **2 hours** of trend, which is what the control
panel draws as sparklines.

Points are serialised as `ago_secs` relative to the request rather than as absolute timestamps, so a
consumer needs no clock agreement with the box.

## `GET /_node/logs?unit=<name>&lines=N`

Tails journald for one unit. It tries the **user** journal first, since these units normally run
under `systemd --user`, and falls back to the system journal. `lines` defaults to 200 and is clamped
to 10–1000.

## Why this is not a shell injection

Two independent guards:

1. **The unit is a whitelist choice, not a parameter.** The requested `unit` must appear in the
   configured `log_units`; anything else is a `400` listing what is available. A request can never
   name an arbitrary journal.
2. **Commands are built argv-style.** `tokio::process::Command` with explicit arguments never invokes
   a shell, so nothing from the request is ever parsed as a command line — no quoting, no escaping, no
   metacharacters to get wrong.

Every command also runs under a hard 4-second timeout with stdin nulled and stderr discarded, and a
non-zero exit yields `None` rather than partial output. Dropping a timed-out command kills its child
process, so repeated timeouts do not leave observability commands running in the background.

## Changelog

- 2026-09-06 — Kill timed-out observability commands when their futures are dropped.
- 2026-09-02 — Added hostname and the configured log-unit list to node status.

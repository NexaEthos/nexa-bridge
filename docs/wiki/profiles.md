# Profiles

A profile is the **operating preset** a workspace chooses for this connection on Agora's Nexa Bridge page
(Settings › Nexa Bridge). It carries the settings Agora sends with every request — window, output, sampling,
thinking, how a forced tool call is phrased — never the model: Agora always sends the exact model your computer
serves, the id it published at connect. Profiles are named by **model family**, never by a checkpoint, so the
same profile fits any size or quantization of that family. Which profile fits the model you serve is your choice;
how selection works is in [Model selection](model-selection.md#your-workspaces-presets).

Nexa builds and certifies the profiles. A computer never creates one, and the Nexa Bridge page shows each
profile's state and its guide for your runtime (steps and serving flags).

## The profiles

| Profile | Family | Input | Window / output | Sampling | Thinking |
| --- | --- | --- | --- | --- | --- |
| **Qwen** | Qwen 3.x with thinking | text | 131,072 / 16,384 | temperature 1, top_p 0.95 | on, budget **512** tokens per call |
| **Qwen High** | Qwen 3.x with thinking | text | 131,072 / 16,384 | temperature 1, top_p 0.95 | on, budget **1,024** tokens per call |
| **Qwen Extra High** | Qwen 3.x with thinking | text | 131,072 / 16,384 | temperature 1, top_p 0.95 | on, budget **2,048** tokens per call |
| **Gemma** | Gemma 4 | text, image | 131,072 / 16,384 | temperature 1, top_p 0.95, top_k 64 | the template's `enable_thinking` switch |
| **Muse Glimmer** | Muse Glimmer | text, image | 131,072 / 16,384 | temperature 1, top_p 0.95 | the vendor's reasoning parser |
| **LFM** | LiquidAI LFM2.5 | text | 131,072 / 16,384 | temperature 0.1 | always on: the vendor template starts reasoning |

## Qwen: a reasoning budget, not a level

The three Qwen profiles are the same profile except for one number, the **reasoning budget**: 512, 1,024 or 2,048
tokens per call. Window, output, sampling and the reasoning level word (`low`) are identical; the budget is what
sets the depth.

On a call with thinking on, Agora sends `chat_template_kwargs: {"enable_thinking": true}`, the level word as
`reasoning_effort` and `thinking_token_budget: <budget>` in the request body. When the budget is reached the runtime
closes the thinking and the model answers: the budget cuts the reasoning, never the answer. The 16,384-token output
window counts the thinking too, so the budget also keeps room for the answer. On the platform's mechanical rounds —
the route choice before a turn — Agora sends `enable_thinking: false` and no budget.

**Why a budget.** On Qwen 3.x the level word alone does not bound how long the model reasons. Measured by replaying a
project's planning round on a Qwen 3.8 27B 4-bit checkpoint served by vLLM, at level `low`:

- without a budget, 14 of 15 runs spent the whole 16,384-token window reasoning and never called the tool;
- with a budget of 2,048, 4,096 or 8,192 tokens, all 10 runs closed the reasoning and called it;
- on AIME 2024 problems the budget changed nothing while unreached (15 of 16 right) and cost accuracy only when set
  far below the model's natural reasoning (256 tokens: 2 of 4 right; 512 and above: 8 of 8).

**Choosing among the three.** The same research project took about 18, 25 and 38 minutes on Qwen, Qwen High and
Qwen Extra High: Qwen High plans and writes more deeply at about one and a half times the time of Qwen, Qwen Extra
High at about twice.

## What your runtime must support

- **Qwen profiles:** vLLM with the `qwen3_coder` tool parser and the `qwen3` reasoning parser, and its reasoning
  configuration so that `thinking_token_budget` is honoured. A runtime that ignores the budget lets the model reason
  unbounded; it is not a fit for these profiles.
- **Gemma:** vLLM 0.32 or later with the `gemma4` tool and reasoning parsers, or LM Studio; the model's own chat
  template reads `enable_thinking`.
- **Muse Glimmer:** vLLM 0.32 or later with the `muse_glimmer` tool and reasoning parsers.
- **LFM:** vLLM with the `lfm2` tool parser and the `qwen3` reasoning parser, keeping the checkpoint's own
  `generation_config` defaults for top_k and repetition_penalty.
- **A forced tool call:** some runtimes, LM Studio among them, refuse a `tool_choice` naming one function. Every
  profile declares the single-tool form: when Agora needs one named tool called, it sends only that tool with
  `tool_choice: "required"`.

Tool calling must be enabled on the model server (vLLM: `--enable-auto-tool-choice` with the family's tool parser);
the Nexa Bridge page shows the exact steps and flags for each runtime.

## A profile that is not here

Want a family that has no profile yet? Join the [NexaEthos Agora Discord](https://discord.gg/k2cFcM2Kc) and propose
it in the **Bridge** section: the model family, the runtime you serve it with and what you use it for. Nexa adds
profiles for new and widely used models as they are tested.

## Changelog

- 2026-10-28 — the page: the six profiles, the Qwen reasoning budget and why it replaces the level, the runtime
  requirements, the forced tool call in single-tool form, where to propose a new profile.

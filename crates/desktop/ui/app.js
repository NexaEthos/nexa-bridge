// Wiki: docs/wiki/desktop.md
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const STATUS_TEXT = {
  stopped: "Not connected",
  starting: "Starting…",
  running: "Connected",
  retrying: "Reconnecting…",
  failed: "Error — restart required",
};

let phase = "stopped";
let busy = false;
let dirty = false;
let initialized = false;
let selectedModel = "";
let guides = [];
let guidesLoading = false;
let guidesScope = "";
let guidesError = "";
let runtimeFlavor = "";
let managedMode = false;
let hasCredentials = false;
let pendingPairing = null;
let nextStep = "";
let confirmWorkspace = "";
let workspaceName = "";
let connectionName = "";
let setupStep = 1;
let compactView = false;
const SETTINGS_FIELDS = ["upstream", "listen", "autostart", "agora-url", "approval-url", "computer-label"];

function initialSetup(state) {
  const credentials = state.has_token && state.has_api_key;
  return {
    compact: state.ready && state.settings.pending_pairing === null,
    step: state.settings.pending_pairing || !credentials ? 1 : state.settings.selected_model ? 3 : 2,
  };
}

function renderSetup() {
  const current = compactView ? 3 : setupStep;
  $("setup-intro").hidden = compactView;
  $("setup-steps").hidden = compactView;
  for (const [step, panel] of [[1, "link-step"], [2, "model-step"], [3, "review-step"]]) {
    $(panel).hidden = step !== current;
    const button = $(`step-${step}`);
    if (step === current) button.setAttribute("aria-current", "step");
    else button.removeAttribute("aria-current");
  }
  $("review-title").textContent = compactView ? "Your connection" : "Review and connect";
  $("review-description").textContent = phase === "running"
    ? "This computer is connected. Keep your model server running to serve requests."
    : "Check the workspace and model before connecting this computer.";
  $("review-workspace").textContent = managedMode ? workspaceName : "Not linked yet";
  $("review-model").textContent = selectedModel || "Not selected yet";
  $("review-preset").textContent = !managedMode ? "Not linked yet"
    : guides.length ? `${guides.map((guide) => guide.name).join(", ")} · on the model this computer serves`
    : "None chosen for this computer yet";
  $("review-guidance").textContent = phase === "failed"
    ? "The connection could not stop cleanly. Quit Bridge from the tray, reopen it and review the error before reconnecting. Settings remain available under Advanced."
    : pendingPairing ? "Complete approval in your browser, or return to Link Agora to cancel and start again."
    : !hasCredentials ? "Return to Link Agora to approve this computer."
    : !selectedModel ? "Choose a model in step 2 before connecting."
    : `This connection belongs to ${workspaceName}${connectionName ? ` · ${connectionName}` : ""}. Only the selected model is shared.`;
  $("connected-guidance").hidden = phase !== "running" || !managedMode;
  $("edit-model").textContent = phase === "stopped" ? "Change model" : "Model settings";
}

function showSetup(step) {
  setupStep = step;
  compactView = false;
  renderSetup();
  $(["link-title", "model-title", "review-title"][step - 1]).focus();
}

function renderControls() {
  const editable = initialized && !busy && phase === "stopped";
  $("toggle").disabled = !initialized || busy || phase === "failed" ||
    (phase === "stopped" && (!hasCredentials || !selectedModel || pendingPairing !== null));
  $("toggle").title = nextStep;
  for (const id of [...SETTINGS_FIELDS, "save", "detect", "detect-address", "link-agora", "open-approval", "cancel-link"]) {
    $(id).disabled = !editable;
  }
  $("link-agora").disabled = !editable || pendingPairing !== null;
  $("refresh-guides").disabled = !initialized || busy || !managedMode || guidesLoading;
  for (const button of $("runtimes").querySelectorAll("button")) {
    button.disabled = !editable;
  }
  for (const button of $("runtime-info").querySelectorAll("button")) {
    button.disabled = !editable;
  }
  $("save").title = phase === "stopped" ? "Save settings" : "Disconnect before changing settings or credentials";
  $("next-step").textContent = dirty && hasCredentials && selectedModel
    ? "Review your connection, then click Connect to save and serve this model." : nextStep;
  renderSetup();
}

function markDirty() {
  dirty = true;
  $("saved").hidden = true;
  renderControls();
}

function renderStatus(status) {
  if (phase !== "running" && status.phase === "running" && !pendingPairing) compactView = true;
  phase = status.phase;
  $("dot").className = `dot ${status.phase}`;
  $("status-line").textContent =
    "Model: " + (status.detail
      ? status.detail
      : STATUS_TEXT[status.phase] ?? status.phase);
  $("status-line").title = status.detail ?? STATUS_TEXT[status.phase] ?? status.phase;
  $("toggle").textContent = status.phase === "stopped"
    ? (confirmWorkspace ? `Connect to ${confirmWorkspace}` : "Connect") : "Disconnect";
  renderControls();
}

function render(state) {
  const wasPending = pendingPairing !== null;
  if (!initialized) {
    const view = initialSetup(state);
    setupStep = view.step;
    compactView = view.compact;
  }
  managedMode = state.settings.managed !== null;
  confirmWorkspace = managedMode && !state.settings.managed.confirmed ? state.settings.managed.tenant_slug : "";
  hasCredentials = state.has_token && state.has_api_key;
  pendingPairing = state.settings.pending_pairing;
  workspaceName = state.settings.managed?.tenant_slug || "";
  connectionName = state.settings.managed?.connection_slug || "";
  if (pendingPairing || !hasCredentials) {
    setupStep = 1;
    compactView = false;
  } else if (wasPending && managedMode) {
    setupStep = 2;
    compactView = false;
  }
  nextStep = state.next_step;
  $("agora-url").value = state.settings.agora_url;
  $("approval-url").value = state.settings.approval_url;
  if (!$("hardware-api").value) $("hardware-api").value = state.settings.agora_url;
  if (!$("hardware-approval").value) $("hardware-approval").value = state.settings.approval_url || state.settings.agora_url;
  $("workspace").textContent = managedMode
    ? `Linked to ${state.settings.managed.tenant_slug}. Credentials are managed automatically.`
    : "Link this computer to Agora. Credentials are handled automatically.";
  $("link-agora").textContent = managedMode ? "Link to Agora again" : "Connect to Agora";
  $("pairing").hidden = pendingPairing === null;
  if (pendingPairing) {
    $("pairing-code").textContent = pendingPairing.code;
    $("pairing-expiry").textContent = `Approval expires at ${new Date(pendingPairing.expires_at * 1000).toLocaleTimeString()}.`;
  }
  $("upstream").value = state.settings.upstream_url;
  selectedModel = state.settings.selected_model;
  renderSelection();
  $("guides-panel").hidden = !managedMode;
  const scope = managedMode ? `${state.settings.agora_url}:${state.settings.managed.device_id}` : "";
  if (scope !== guidesScope) {
    guidesScope = scope;
    guides = [];
    guidesError = "";
    if (scope) queueMicrotask(refreshGuides);
  }
  renderGuides();
  $("listen").value = state.settings.listen_addr;
  $("autostart").checked = state.settings.autostart;
  dirty = false;
  initialized = true;
  renderStatus(state.status);
  renderBuild(state.build);
}

function renderBuild(build) {
  const origin = build.agora_origin ? ` · ${build.agora_origin}` : "";
  $("build").textContent =
    `Nexa Bridge ${build.version} · ${build.commit} · ${build.target} · ${build.profile} · ${build.channel}${origin}`;
}

function showError(message) {
  const el = $("error");
  el.textContent = message;
  el.hidden = !message;
}

async function save() {
  if (busy || phase !== "stopped" || !initialized) return false;
  busy = true;
  renderControls();
  showError("");
  try {
    const state = await invoke("save", {
      req: {
        settings: {
          agora_url: $("agora-url").value.trim(),
          approval_url: $("approval-url").value.trim(),
          upstream_url: $("upstream").value.trim(),
          selected_model: selectedModel,
          listen_addr: $("listen").value.trim(),
          relay_url: "",
          autostart: $("autostart").checked,
        },
      },
    });
    render(state);
    const saved = $("saved");
    saved.hidden = false;
    setTimeout(() => (saved.hidden = true), 1600);
    return true;
  } catch (e) {
    showError(String(e));
    markDirty();
    return false;
  } finally {
    busy = false;
    renderControls();
  }
}

async function toggle() {
  if ($("toggle").disabled) return;
  if (phase === "stopped" && dirty && !await save()) return;
  busy = true;
  renderControls();
  showError("");
  try {
    render(await invoke("set_running", { running: phase === "stopped" }));
  } catch (e) {
    showError(String(e));
  } finally {
    busy = false;
    renderControls();
  }
}

function renderLinkState(state) {
  const unsaved = dirty ? {
    model: selectedModel,
    fields: Object.fromEntries(SETTINGS_FIELDS.filter((id) => !["agora-url", "approval-url"].includes(id))
      .map((id) => [id, { value: $(id).value, checked: $(id).checked }])),
  } : null;
  render(state);
  if (unsaved) {
    selectedModel = unsaved.model;
    for (const [id, value] of Object.entries(unsaved.fields)) {
      $(id).value = value.value;
      if (value.checked !== undefined) $(id).checked = value.checked;
    }
    renderSelection();
    renderGuides();
    markDirty();
  }
}

$("link-agora").addEventListener("click", async () => {
  if (busy || phase !== "stopped") return;
  if (!$("agora-url").value.trim()) {
    $("show-advanced").checked = true;
    $("agora-url").focus();
    showError("This installer has no Agora address. Enter the HTTPS server address supplied by your workspace.");
    return;
  }
  busy = true;
  renderControls();
  showError("");
  try {
    renderLinkState(await invoke("begin_link", { agoraUrl: $("agora-url").value.trim(), approvalUrl: $("approval-url").value.trim(), label: $("computer-label").value.trim() }));
  } catch (error) {
    try {
      renderLinkState(await invoke("get_state"));
      showError(String(error));
    } catch (refreshError) {
      showError(`${error}; cannot reload linking state: ${refreshError}`);
    }
  } finally {
    busy = false;
    renderControls();
  }
});

$("open-approval").addEventListener("click", () => {
  invoke("open_approval").catch((error) => showError(String(error)));
});

$("cancel-link").addEventListener("click", async () => {
  if (busy || phase !== "stopped") return;
  busy = true;
  renderControls();
  try {
    renderLinkState(await invoke("cancel_link"));
    showError("Linking canceled on this computer. If it was already approved, revoke that unused device in Agora.");
  } catch (error) {
    showError(String(error));
  } finally {
    busy = false;
    renderControls();
  }
});

// Keep polling after transient errors; Rust clears final approval refusals.
const POLL_MIN_MS = 5000;
const POLL_MAX_MS = 60000;
let pollDelay = POLL_MIN_MS;
async function pollApproval() {
  if (pendingPairing && !busy && phase === "stopped") {
    busy = true;
    try {
      renderLinkState(await invoke("poll_link"));
      if (!pendingPairing) showError("");
      pollDelay = POLL_MIN_MS;
    } catch (error) {
      showError(String(error));
      pollDelay = Math.min(pollDelay * 2, POLL_MAX_MS);
      try {
        renderLinkState(await invoke("get_state"));
      } catch (refreshError) {
        showError(`${error}; cannot refresh approval state: ${refreshError}`);
      }
    } finally {
      busy = false;
      renderControls();
    }
  }
  setTimeout(pollApproval, pollDelay);
}
setTimeout(pollApproval, POLL_MIN_MS);

const CONCURRENCY_LABEL = {
  parallel: "handles parallel requests",
  serialized: "one request at a time",
  unknown: "concurrency unknown",
};

function clear(el) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

const RUNTIME_GUIDES = {
  "lm-studio": ["lm-studio"],
  ollama: ["ollama"],
  "llama-cpp": ["llama-cpp"],
  vllm: ["vllm"],
  "open-ai-compatible": ["sglang", "open-ai-compatible"],
};
const RUNTIME_LABEL = {
  "lm-studio": "LM Studio",
  ollama: "Ollama",
  "llama-cpp": "llama.cpp",
  vllm: "vLLM",
  sglang: "SGLang",
  "open-ai-compatible": "Other OpenAI-compatible server",
};

function line(tag, className, text) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  el.textContent = text;
  return el;
}

function guideCard(guide) {
  const card = line("div", "guide", "");
  const head = line("div", "guide-head", "");
  head.append(
    line("strong", "", guide.name),
    line("span", "badge", guide.certified ? "certified by Nexa" : "in testing"),
  );
  card.append(head);
  if (guide.context_window) {
    card.append(line("p", "hint",
      `Its context window is ${guide.context_window.toLocaleString()} tokens: a longer request is refused by your model server.`));
  }
  if (guide.guide?.notes) card.append(line("p", "note", guide.guide.notes));
  const runtimes = guide.guide?.runtimes ?? {};
  const keys = runtimeFlavor ? (RUNTIME_GUIDES[runtimeFlavor] ?? []) : Object.keys(runtimes);
  for (const key of keys) {
    const steps = runtimes[key];
    if (!steps) continue;
    card.append(line("p", "hint", RUNTIME_LABEL[key] ?? key));
    if (steps.steps.length) {
      const list = document.createElement("ol");
      for (const step of steps.steps) list.append(line("li", "", step));
      card.append(list);
    }
    if (steps.args.length) card.append(line("pre", "", steps.args.join(" ")));
  }
  return card;
}

function renderGuides() {
  const box = $("guides");
  clear(box);
  for (const guide of guides) box.append(guideCard(guide));
  $("guides-status").textContent = guidesLoading ? "Loading your workspace's presets…"
    : guidesError
    || (!guides.length
      ? "Your workspace has not chosen a preset for this computer yet. A workspace administrator chooses the operating preset for this connection in Agora's model settings."
      : "");
}

async function refreshGuides() {
  if (!managedMode || guidesLoading) return;
  const scope = guidesScope;
  guidesLoading = true;
  guidesError = "";
  renderGuides();
  renderControls();
  try {
    const found = await invoke("get_guides");
    if (scope === guidesScope) guides = found;
  } catch (error) {
    if (scope === guidesScope) {
      guides = [];
      guidesError = `Cannot load your workspace's models: ${error}`;
    }
  } finally {
    guidesLoading = false;
    renderGuides();
    renderControls();
    if (scope !== guidesScope && managedMode) void refreshGuides();
  }
}

$("refresh-guides").addEventListener("click", refreshGuides);

function renderSelection() {
  $("selection").textContent = selectedModel
    ? `Selected: ${selectedModel}. Only this model will be shared when you connect.`
    : "No model selected. Find local models, then choose one.";
  for (const button of $("runtime-info").querySelectorAll("button[data-model]")) {
    const selected = button.dataset.model === selectedModel;
    button.setAttribute("aria-pressed", String(selected));
    button.textContent = selected ? `${button.dataset.model} — Selected` : button.dataset.model;
  }
}

function showRuntime(rt) {
  const box = $("runtime-info");
  clear(box);
  runtimeFlavor = rt.flavor;
  renderGuides();

  const head = document.createElement("div");
  head.className = "info-head";
  const name = document.createElement("strong");
  name.textContent = rt.label;
  const badge = document.createElement("span");
  badge.className = `badge ${rt.concurrency}`;
  badge.textContent = CONCURRENCY_LABEL[rt.concurrency] ?? rt.concurrency;
  head.append(name, badge);
  box.append(head);

  const loadStateKnown = rt.loaded_models !== null;
  const visibleModels = loadStateKnown ? rt.loaded_models : rt.models;
  const modelLabel = document.createElement("p");
  modelLabel.className = "hint";
  modelLabel.textContent = loadStateKnown
    ? `Loaded models (${visibleModels.length}) — at last detection`
    : "Reported models — loading state unavailable";
  box.append(modelLabel);

  if (visibleModels.length) {
    const models = document.createElement("div");
    models.className = "models";
    for (const m of visibleModels) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.textContent = m;
      chip.dataset.model = m;
      chip.title = "Select this model for exclusive serving";
      chip.addEventListener("click", () => {
        if (busy || phase !== "stopped") return;
        selectedModel = m;
        renderGuides();
        renderSelection();
        markDirty();
      });
      models.append(chip);
    }
    box.append(models);
  } else {
    const empty = document.createElement("p");
    empty.className = "note";
    empty.textContent = loadStateKnown
      ? "No model loaded. Load one in LM Studio, start its local server, then click Find local models again."
      : "Server is running, but reports no models.";
    box.append(empty);
  }

  if (loadStateKnown) {
    const otherCount = rt.models.filter((model) => !visibleModels.includes(model)).length;
    if (otherCount > 0) {
      const other = document.createElement("p");
      other.className = "note";
      other.textContent = `${otherCount} other model${otherCount === 1 ? "" : "s"} listed; not reported as loaded.`;
      box.append(other);
    }
  }

  if (rt.note) {
    const note = document.createElement("p");
    note.className = "note";
    note.textContent = rt.note;
    box.append(note);
  }
  box.hidden = false;
  renderSelection();
  renderControls();
}

function showChoices(found) {
  const box = $("runtimes");
  clear(box);
  for (const rt of found) {
    const row = document.createElement("button");
    row.type = "button";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = rt.label;
    const count = document.createElement("span");
    count.textContent = rt.loaded_models === null
      ? `${rt.models.length} reported`
      : `${rt.loaded_models.length} loaded`;
    count.className = "hint";
    const where = document.createElement("span");
    where.className = "where";
    where.textContent = rt.url.replace(/^https?:\/\//, "");
    row.append(name, count, where);
    row.addEventListener("click", () => {
      if (busy || phase !== "stopped") return;
      box.hidden = true;
      apply(rt);
    });
    box.append(row);
  }
  box.hidden = found.length === 0;
}

function apply(rt) {
  if ($("upstream").value.trim().replace(/\/$/, "") !== rt.url) {
    selectedModel = "";
    renderGuides();
  }
  $("upstream").value = rt.url;
  markDirty();
  showRuntime(rt);
}

async function detect(useAddress = false) {
  if (busy || phase !== "stopped" || !initialized) return;
  busy = true;
  renderControls();
  const button = $(useAddress ? "detect-address" : "detect");
  showSetup(2);
  button.disabled = true;
  button.textContent = "Scanning…";
  showError("");
  $("runtimes").hidden = true;
  $("runtime-info").hidden = true;
  try {
    const typed = $("upstream").value.trim();
    if (useAddress) {
      apply(await invoke("probe_runtime", { url: typed }));
      return;
    }
    const found = await invoke("detect_runtimes");
    if (found.length === 1) {
      apply(found[0]);
    } else if (found.length > 1) {
      showChoices(found);
    } else {
      showError("No local model server found. In LM Studio, load a model and start the server in Developer. For Ollama, start the app. Then try Find local models again, or check a custom server address in Advanced.");
    }
  } catch (e) {
    showError(String(e));
  } finally {
    busy = false;
    renderControls();
    button.textContent = useAddress ? "Check address" : "Find local models";
  }
}

for (const step of [1, 2, 3]) $(`step-${step}`).addEventListener("click", () => showSetup(step));
for (const [id, step] of [["continue-model", 2], ["continue-review", 3], ["back-link", 1], ["edit-model", 2], ["edit-workspace", 1]]) {
  $(id).addEventListener("click", () => showSetup(step));
}
$("detect").addEventListener("click", () => detect());
$("detect-address").addEventListener("click", () => detect(true));
$("save").addEventListener("click", save);
$("toggle").addEventListener("click", toggle);

for (const id of SETTINGS_FIELDS) $(id).addEventListener("input", markDirty);
$("upstream").addEventListener("input", () => {
  selectedModel = "";
  runtimeFlavor = "";
  $("runtime-info").hidden = true;
  $("runtimes").hidden = true;
  renderSelection();
  renderGuides();
  renderControls();
});

renderControls();
listen("status", (event) => renderStatus(event.payload))
  .then(() => invoke("get_state"))
  .then(render)
  .catch((e) => showError(String(e)));

$("diagnostics").addEventListener("click", async () => {
  $("diagnostics-note").textContent = "";
  try {
    const path = await invoke("export_diagnostics");
    $("diagnostics-note").textContent = `Saved ${path}`;
  } catch (error) {
    showError(String(error));
  }
});

function renderUpdate(update) {
  $("update").hidden = !update;
  if (!update) return;
  $("update-text").textContent = `Nexa Bridge ${update.version} is available — this computer runs ${update.current}.`;
  $("update-notes").textContent = update.notes ?? "";
  $("update-details").hidden = !update.notes;
}

async function checkUpdate() {
  $("check-update").disabled = true;
  $("update-note").textContent = "Checking…";
  try {
    const update = await invoke("check_update");
    renderUpdate(update);
    $("update-note").textContent = update ? "" : "This is the latest version.";
  } catch (error) {
    $("update-note").textContent = String(error);
  } finally {
    $("check-update").disabled = false;
  }
}

$("check-update").addEventListener("click", checkUpdate);
$("install-update").addEventListener("click", async () => {
  $("install-update").disabled = true;
  $("update-text").textContent = "Downloading the update. The connection stops and the app restarts when it is installed.";
  try {
    await invoke("install_update");
  } catch (error) {
    showError(String(error));
    $("install-update").disabled = false;
  }
});
checkUpdate();

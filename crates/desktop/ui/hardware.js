// Wiki: docs/wiki/desktop.md
(() => {
  const { invoke } = window.__TAURI__.core;
  const el = (id) => document.getElementById(id);
  let busy = false;
  let loaded = false;
  let schedulerError = "";

  function error(message) {
    const detail = [message, schedulerError].filter(Boolean).join(" ");
    el("hardware-error").textContent = detail;
    el("hardware-error").hidden = !detail;
  }

  function controls() {
    for (const button of el("devices-panel").querySelectorAll("button")) button.disabled = busy;
    for (const input of el("devices-panel").querySelectorAll("input, select")) input.disabled = busy;
    el("hardware-inspect").disabled = busy || !el("hardware-serial").value;
    el("hardware-pair").disabled = busy || !el("hardware-serial").value;
  }

  async function operation(run) {
    if (busy) return;
    busy = true;
    error("");
    controls();
    try {
      await run();
    } catch (failure) {
      error(String(failure));
    } finally {
      busy = false;
      controls();
    }
  }

  function text(tag, value, className = "") {
    const item = document.createElement(tag);
    item.textContent = value;
    item.className = className;
    return item;
  }

  function date(seconds) {
    return new Date(seconds * 1000).toLocaleString();
  }

  async function act(card, action) {
    await operation(async () => {
      try {
        render(await invoke("hardware_action", { localId: card.local_id, action }));
      } catch (failure) {
        await readState();
        throw failure;
      }
    });
  }

  function confirmAction(message) {
    const dialog = el("hardware-confirm");
    el("hardware-confirm-text").textContent = message;
    dialog.returnValue = "cancel";
    return new Promise((resolve) => {
      dialog.addEventListener("close", () => resolve(dialog.returnValue === "confirm"), { once: true });
      dialog.showModal();
    });
  }

  function button(label, card, action, confirmation) {
    const item = text("button", label, "ghost");
    item.type = "button";
    item.addEventListener("click", async () => {
      if (confirmation && !await confirmAction(confirmation)) return;
      void act(card, action);
    });
    return item;
  }

  function render(cards) {
    const list = el("hardware-cards");
    list.replaceChildren();
    if (!cards.length) {
      list.append(text("p", "No boards linked on this computer yet. Inspect a USB board above, then connect it to your workspace.", "empty-state"));
    }
    for (const card of cards) {
      const item = document.createElement("article");
      item.className = "device-card";
      const heading = document.createElement("div");
      heading.className = "section-heading";
      heading.append(text("h3", card.label), text("span", card.automatic ? "Every 15 minutes" : "Automatic sends paused", "badge"));
      item.append(heading, text("p", `Arduino Q · board.observe · USB ${card.serial}`, "hint"));
      item.append(text("p", card.workspace ? `Workspace: ${card.workspace}` : "Workspace approval not confirmed on this computer.", "workspace-name"));
      if (card.pairing_code) {
        item.append(text("p", `Approval code: ${card.pairing_code}`, "pairing-code"));
        item.append(text("p", `Expires: ${date(card.expires_at)}. Approve in the browser, then check approval here.`, "hint"));
      }
      if (card.receipt) {
        item.append(text("p", `Last receipt: ${card.receipt.status} · ${new Date(card.receipt.received_at).toLocaleString()}`, "receipt"));
        const details = document.createElement("details");
        details.append(text("summary", "Receipt details"), text("p", `Event ${card.receipt.event_id} · Journal ${card.receipt.journal_id}`, "hint"));
        item.append(details);
      } else {
        item.append(text("p", "No observation receipt yet.", "hint"));
      }
      if (card.automatic) item.append(text("p", `Next observation: ${date(Math.max(card.next_observation, card.retry_not_before))}`, "hint"));
      if (card.pending) item.append(text("p", "One observation is saved locally for retry. Retry sends the same event ID.", "info"));
      if (card.retry_not_before > Date.now() / 1000) item.append(text("p", `Server requested a wait until ${date(card.retry_not_before)}.`, "hint"));
      if (card.error) item.append(text("p", card.error, "error"));
      const actions = document.createElement("div");
      actions.className = "device-actions";
      actions.append(button("Check approval", card, "check"));
      if (card.pairing_code) actions.append(button("Open approval", card, "open_approval"));
      if (card.workspace) {
        actions.append(button(card.pending ? "Retry pending observation" : "Send an observation now", card, "send",
          `Send board diagnostics to ${card.workspace}? Only Linux diagnostics are collected; no commands or pin changes.`));
        actions.append(button(card.automatic ? "Pause automatic sends" : "Enable every 15 minutes", card,
          card.automatic ? "pause" : "enable_automatic", card.automatic ? null :
            `Send this board's diagnostics to ${card.workspace} every 15 minutes while Bridge is running? You can pause at any time.`));
      }
      actions.append(button("Manage / revoke in Agora", card, "manage"));
      actions.append(button("Remove from this computer", card, "forget",
        "Remove the local record and its hardware key? This stops local sends but does not revoke server access. Revoke the endpoint in Agora first if it was approved." +
        (card.pending ? " The pending observation will be archived locally and shown in your file manager. It will no longer be retried; the server may already have received it." : "")));
      item.append(actions);
      list.append(item);
    }
    controls();
  }

  async function readState() {
    const state = await invoke("hardware_state");
    schedulerError = state.scheduler_error || "";
    render(state.cards);
    error("");
    loaded = true;
  }

  async function refresh() {
    await operation(readState);
  }

  for (const page of ["models", "devices"]) {
    el(`tab-${page}`).addEventListener("click", () => {
      for (const name of ["models", "devices"]) {
        el(`${name}-panel`).hidden = name !== page;
        el(`tab-${name}`).setAttribute("aria-pressed", String(name === page));
      }
      if (page === "devices") void refresh();
    });
  }

  el("hardware-refresh").addEventListener("click", refresh);
  el("hardware-serial").addEventListener("change", () => {
    el("hardware-snapshot").hidden = true;
    controls();
  });
  el("hardware-scan").addEventListener("click", () => operation(async () => {
    const select = el("hardware-serial");
    select.replaceChildren(new Option("Select your board", ""));
    el("hardware-snapshot").hidden = true;
    const devices = await invoke("hardware_inventory");
    for (const device of devices) {
      const option = new Option(`${device.model || "ADB device"} · ${device.serial} · ${device.state}`, device.serial);
      option.disabled = device.state !== "device";
      select.add(option);
    }
    el("hardware-inventory-note").textContent = devices.length
      ? "Choose your board explicitly. For an unauthorized or offline device, unlock/authorize USB access and reconnect, then find boards again."
      : "No ADB devices found. Connect your Arduino Q with a data-capable USB cable, then try again.";
  }));

  el("hardware-inspect").addEventListener("click", () => operation(async () => {
    const snapshot = await invoke("hardware_inspect", { serial: el("hardware-serial").value });
    const panel = el("hardware-snapshot");
    panel.replaceChildren(text("strong", snapshot.board_model));
    const readings = document.createElement("dl");
    for (const [name, value] of [
      ["Kernel", snapshot.kernel], ["Architecture", snapshot.architecture],
      ["Uptime", `${snapshot.uptime_seconds.toLocaleString()} seconds`],
      ["Available memory", `${snapshot.memory_available_bytes.toLocaleString()} bytes`],
      ["Total memory", `${snapshot.memory_total_bytes.toLocaleString()} bytes`],
      ["Arduino router", `${snapshot.router_version} · ${snapshot.router_state}`],
    ]) readings.append(text("dt", name), text("dd", value));
    panel.append(readings, text("p", "Local inspection only. Nothing was sent to Agora.", "hint"));
    panel.hidden = false;
  }));

  el("hardware-pair").addEventListener("click", () => operation(async () => {
    const request = {
      api_origin: el("hardware-api").value.trim(), approval_origin: el("hardware-approval").value.trim(),
      serial: el("hardware-serial").value, label: el("hardware-label").value.trim(),
    };
    if (!request.label || !request.api_origin || !request.approval_origin) throw new Error("Enter both trusted origins and an asset name before connecting.");
    try {
      render(await invoke("hardware_begin", { request }));
    } catch (failure) {
      await readState();
      throw failure;
    }
  }));

  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && loaded && !el("devices-panel").hidden && !busy) void refresh();
  });
  controls();
})();

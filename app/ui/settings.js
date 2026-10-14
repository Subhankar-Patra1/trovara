// Trovara's settings (tray: Settings…). Every change is saved at once and the helpers follow it; the shortcut and fast
// catch-up have their own commands (the shortcut is tried before it's kept; fast catch-up asks Windows for permission).
"use strict";

const tauri = window.__TAURI__;
const invoke = (command, args) => tauri.core.invoke(command, args);
const $ = (id) => document.getElementById(id);

const PRIORITY_NOTES = {
  auto: "Full speed on mains power, most efficient on battery. Recommended.",
  normal: "Full speed always. Indexing is GPU work, so your apps stay quick.",
  low: "Gives way to other programs. Indexing takes several times longer.",
  eco: "Windows' efficiency mode: the least power, the slowest indexing.",
};

let settings = null;

async function start() {
  const info = await invoke("get_settings");
  settings = info.settings;
  settings.folders = info.folders;
  TrovaraHotkey.attach({ keys: $("keys"), change: $("change-key"), reset: $("reset-key"), note: $("key-note"),
                         current: info.hotkey });
  $("version").textContent = `Trovara ${info.version}`;
  if (!info.updates) showUpdate({ state: "off" });
  $("index-place").textContent = `Index: ${info.index}`;
  showModel(info.model);
  tauri.event.listen("model", ({ payload }) => showModel(payload));
  tauri.event.listen("paused", ({ payload }) => { settings.paused = payload; $("paused").checked = payload; });
  render();
  checkCatchUp();
}

function render() {
  renderCode();
  renderPaths($("folder-list"), settings.folders, "No folders: Trovara searches nothing yet.", (path) => {
    settings.folders = settings.folders.filter((folder) => folder !== path);
    save();
  });
  renderPaths($("skipped-list"), settings.skipped, "No folders left out.", (path) => {
    settings.skipped = settings.skipped.filter((folder) => folder !== path);
    save();
  });
  $("skip-types").value = settings.skipTypes.join(", ");
  $("paused").checked = settings.paused;
  $("on-battery").checked = settings.indexOnBattery;
  $("pause-fullscreen").checked = settings.pauseFullscreen;
  const [from, to] = (settings.hours || "").split("-");
  $("hours-on").checked = Boolean(settings.hours);
  $("hours").hidden = !settings.hours;
  $("hours-from").value = from || "22:00";
  $("hours-to").value = to || "07:00";
  $("priority").value = settings.priority || "auto";
  $("priority-note").textContent = PRIORITY_NOTES[$("priority").value];
  $("launch-at-login").checked = settings.launchAtLogin;
  $("check-updates").checked = settings.checkUpdates;
}

function renderPaths(container, paths, emptyText, remove) {
  if (!paths.length) {
    const line = document.createElement("div");
    line.className = "line-item";
    const empty = document.createElement("span");
    empty.className = "empty";
    empty.textContent = emptyText;
    line.append(empty);
    container.replaceChildren(line);
    return;
  }
  container.replaceChildren(...paths.map((path) => {
    const line = document.createElement("div");
    line.className = "line-item";
    const name = document.createElement("span");
    name.className = "path";
    name.textContent = path.replaceAll("/", "\\");
    name.title = name.textContent;
    const button = document.createElement("button");
    button.className = "remove";
    button.textContent = "Remove";
    button.onclick = () => remove(path);
    line.append(name, button);
    return line;
  }));
}

let saving = Promise.resolve();
function save() {
  render();
  saving = saving.then(async () => {
    try {
      await invoke("save_settings", { settings });
      $("saved").textContent = "Saved";
      setTimeout(() => { $("saved").textContent = ""; }, 1500);
    } catch (error) {
      $("saved").textContent = String(error);
    }
  });
}

$("add-folder").addEventListener("click", async () => {
  const picked = await invoke("pick_folders");
  const known = new Set(settings.folders.map((path) => path.toLowerCase()));
  settings.folders.push(...picked.filter((path) => !known.has(path.toLowerCase())));
  if (picked.length) save();
});

$("add-skipped").addEventListener("click", async () => {
  const picked = await invoke("pick_folders");
  const known = new Set(settings.skipped.map((path) => path.toLowerCase()));
  settings.skipped.push(...picked.filter((path) => !known.has(path.toLowerCase())));
  if (picked.length) save();
});

$("skip-types").addEventListener("change", () => {
  settings.skipTypes = $("skip-types").value.split(/[\s,;]+/).map((ext) => ext.replace(/^\./, "").toLowerCase()).filter(Boolean);
  save();
});

const toggles = { paused: "paused", "on-battery": "indexOnBattery", "pause-fullscreen": "pauseFullscreen",
                  "launch-at-login": "launchAtLogin", "check-updates": "checkUpdates" };
for (const [id, key] of Object.entries(toggles)) {
  $(id).addEventListener("change", () => {
    settings[key] = $(id).checked;
    save();
  });
}

function saveHours() {
  settings.hours = $("hours-on").checked ? `${$("hours-from").value || "22:00"}-${$("hours-to").value || "07:00"}` : "";
  save();
}
$("hours-on").addEventListener("change", saveHours);
$("hours-from").addEventListener("change", saveHours);
$("hours-to").addEventListener("change", saveHours);

$("priority").addEventListener("change", () => {
  settings.priority = $("priority").value;
  save();
});

// MARK: Fast catch-up

async function checkCatchUp() {
  const button = $("catchup");
  try {
    const on = await invoke("fast_catch_up");
    $("catchup-note").textContent = on
      ? "On: after a restart Trovara asks Windows' change journal what changed (well under a second)."
      : "Off: after a restart Trovara looks through every folder. Turning it on asks for administrator permission once.";
    button.textContent = on ? "Turn Off" : "Turn On";
    button.disabled = false;
    button.onclick = async () => {
      button.disabled = true;
      $("catchup-note").textContent = "Waiting for Windows' permission…";
      try {
        await invoke("set_fast_catch_up", { on: !on });
      } catch (error) {
        $("catchup-note").textContent = String(error);
        button.disabled = false;
        return;
      }
      checkCatchUp();
    };
  } catch (error) {
    $("catchup-note").textContent = String(error);
  }
}

// MARK: The model

function showModel(model) {
  const lines = { checking: "Checking…", ready: "Ready. Everything runs on this PC.",
                  failed: `Couldn't download: ${model.message || "unknown error"}` };
  let text = lines[model.state] || "";
  if (model.state === "downloading") {
    text = model.total && model.bytes ? `Downloading · ${Math.floor((100 * model.bytes) / model.total)}%` : "Downloading…";
    if (model.problem) text += ` (${model.problem}; trying again)`;
  }
  $("model").textContent = text;
  $("model-retry").hidden = model.state !== "failed";
  $("model-files").hidden = model.state !== "failed" && model.state !== "downloading";
}

$("model-retry").addEventListener("click", () => invoke("retry_model").catch(() => {}));
$("model-files").addEventListener("click", () => invoke("use_model_files").catch(() => {}));
$("open-logs").addEventListener("click", () => invoke("open_logs").catch(() => {}));
$("licenses").addEventListener("click", () => invoke("open_acknowledgements").catch((error) => { $("saved").textContent = String(error); }));

// MARK: Updates

$("check-now").addEventListener("click", async () => {
  const button = $("check-now");
  button.disabled = true;
  $("update-note").textContent = "Checking…";
  try {
    showUpdate(await invoke("check_updates"));
  } catch (error) {
    $("update-note").textContent = String(error);
  }
  button.disabled = false;
});

function showUpdate(answer) {
  const note = $("update-note");
  if (answer.state === "off") {
    note.textContent = answer.message || "This build doesn't check for updates.";
    $("check-now").hidden = true;
  } else if (answer.state === "current") {
    note.textContent = `Trovara ${answer.version} is the newest version.`;
  } else if (answer.state === "busy") {
    note.textContent = "Already checking…";
  } else if (answer.state === "ready") {
    note.textContent = `Trovara ${answer.version} is downloaded. `;
    const restart = document.createElement("button");
    restart.className = "link";
    restart.textContent = "Restart to Update";
    restart.onclick = () => invoke("restart_to_update").catch((error) => { note.textContent = String(error); });
    note.append(restart);
  }
}
tauri.event.listen("update", ({ payload }) => showUpdate(payload));
$("done").addEventListener("click", async () => {
  await saving;
  invoke("close_window").catch(() => window.close());
});

// MARK: Code

const codeEstimates = new Map();   // folder → its line ("312 files · about 2 min · 4 MB"), once estimated

function renderCode() {
  const code = settings.code;
  $("code-on").checked = code.on;
  $("code-parts").hidden = !code.on;
  $("code-buttons").hidden = !code.on;
  $("code-skipped-card").hidden = !code.on || !code.skipped.length;
  if (!code.on) return;
  renderPaths($("code-folder-list"), code.folders, "No code folders yet: add a repo, or a folder of them.", (path) => {
    code.folders = code.folders.filter((folder) => folder !== path);
    save();
  });
  // Each folder says what reading it takes (PRD F-COD-07).
  for (const [index, path] of code.folders.entries()) {
    const line = $("code-folder-list").children[index];
    const note = document.createElement("span");
    note.className = "label";
    note.style.flex = "none";
    note.innerHTML = `<span>${codeEstimates.get(path) ?? "Counting…"}</span>`;
    line?.insertBefore(note, line.lastChild);
  }
  renderPaths($("code-skipped-list"), code.skipped, "", (path) => {
    code.skipped = code.skipped.filter((folder) => folder !== path);
    save();
  });
}

async function estimateCode(paths) {
  const wanted = paths.filter((path) => !codeEstimates.has(path));
  if (!wanted.length) return;
  try {
    const answers = await invoke("code_estimate", { folders: wanted });
    for (const path of wanted) {
      const answer = answers.find((item) => (item.folder || "").toLowerCase().replaceAll("/", "\\") === path.toLowerCase().replaceAll("/", "\\"));
      codeEstimates.set(path, answer ? codeLine(answer) : "");
    }
  } catch (error) {
    for (const path of wanted) codeEstimates.set(path, "");
  }
  renderCode();
}

/// "312 files · about 2 min · 4 MB"
function codeLine(answer) {
  if (answer.readable === false) return "Can't read it";
  const files = answer.files?.code ?? 0;
  if (!files) return "No code Trovara reads in it";
  const seconds = Number(answer.seconds || 0);
  const time = seconds < 90 ? `${Math.max(1, Math.round(seconds))} s` : seconds < 5400 ? `${Math.round(seconds / 60)} min`
    : `${(seconds / 3600).toFixed(1)} h`;
  const megabytes = Number(answer.bytes || 0) / 1e6;
  const room = megabytes < 1 ? "under 1 MB" : `${megabytes < 10 ? megabytes.toFixed(1) : Math.round(megabytes)} MB`;
  return `${files.toLocaleString()} ${files === 1 ? "file" : "files"} · about ${time} · ${room}`;
}

async function loadEditors() {
  try {
    const info = await invoke("code_editors");
    const select = $("editor");
    const automatic = document.createElement("option");
    automatic.value = "";
    automatic.textContent = info.opensIn === "its app" ? "Automatic (the file's app)" : `Automatic (${info.opensIn})`;
    select.replaceChildren(automatic, ...info.installed.map((editor) => {
      const option = document.createElement("option");
      option.value = editor.id;
      option.textContent = editor.name;
      return option;
    }));
    select.value = info.installed.some((editor) => editor.id === settings.code.editor) ? settings.code.editor : "";
    if (!info.installed.length) $("editor-note").textContent = "No code editor found: code opens in the file's own app.";
  } catch (error) {
    $("editor-note").textContent = String(error);
  }
}

$("code-on").addEventListener("change", () => {
  if (!$("code-on").checked && settings.code.on) {
    // Turning it off deletes the code index: that asks first.
    $("code-on").checked = true;
    $("code-off").click();
    return;
  }
  settings.code.on = true;
  save();
  if (!settings.code.folders.length) $("add-code-folder").click();
});

$("add-code-folder").addEventListener("click", async () => {
  const picked = await invoke("pick_folders");
  const known = new Set(settings.code.folders.map((path) => path.toLowerCase()));
  const added = picked.filter((path) => !known.has(path.toLowerCase()));
  settings.code.folders.push(...added);
  if (added.length) {
    save();
    estimateCode(added);
  }
});

$("add-code-skipped").addEventListener("click", async () => {
  const picked = await invoke("pick_folders");
  const known = new Set(settings.code.skipped.map((path) => path.toLowerCase()));
  settings.code.skipped.push(...picked.filter((path) => !known.has(path.toLowerCase())));
  if (picked.length) save();
});

$("editor").addEventListener("change", () => {
  settings.code.editor = $("editor").value;
  save();
});

$("code-off").addEventListener("click", async () => {
  try {
    if (await invoke("code_off")) {
      settings.code.on = false;
      render();
    }
  } catch (error) {
    $("saved").textContent = String(error);
  }
});

start().then(() => {
  loadEditors();
  if (settings.code.on) estimateCode(settings.code.folders);
});

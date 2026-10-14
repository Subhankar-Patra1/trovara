// Trovara's settings (tray: Settings…). Every change is saved at once and the helpers follow it; the shortcut and fast
// catch-up have their own commands (the shortcut is tried before it's kept; fast catch-up asks Windows for permission).
"use strict";

const tauri = window.__TAURI__;
const invoke = (command, args) => tauri.core.invoke(command, args);
const $ = (id) => document.getElementById(id);

// Keep one section visible and retain its scroll position while navigating.
const tabs = [...document.querySelectorAll(".nav-item")];
const sectionScroll = new Map();
let activeSection = "general";
function selectSection(tab) {
  if (activeSection === "general" && tab.dataset.section !== "general" && $("keys").classList.contains("recording")) {
    $("change-key").click();
  }
  const content = $("settings-content");
  sectionScroll.set(activeSection, content.scrollTop);
  activeSection = tab.dataset.section;
  for (const item of tabs) {
    const selected = item === tab;
    item.classList.toggle("active", selected);
    item.setAttribute("aria-selected", String(selected));
    item.tabIndex = selected ? 0 : -1;
    $(item.dataset.section).hidden = !selected;
  }
  const page = $(activeSection);
  $("page-title").textContent = page.dataset.title;
  $("page-description").textContent = page.dataset.description;
  content.scrollTop = sectionScroll.get(activeSection) || 0;
  tab.scrollIntoView({ block: "nearest", inline: "nearest" });
}
for (const tab of tabs) {
  tab.addEventListener("click", () => selectSection(tab));
  tab.addEventListener("keydown", (event) => {
    const horizontal = window.matchMedia("(max-width: 620px)").matches;
    const nextKey = horizontal ? "ArrowRight" : "ArrowDown";
    const previousKey = horizontal ? "ArrowLeft" : "ArrowUp";
    let index = tabs.indexOf(tab);
    if (event.key === nextKey) index = (index + 1) % tabs.length;
    else if (event.key === previousKey) index = (index + tabs.length - 1) % tabs.length;
    else if (event.key === "Home") index = 0;
    else if (event.key === "End") index = tabs.length - 1;
    else return;
    event.preventDefault();
    selectSection(tabs[index]);
    tabs[index].focus();
  });
}
const narrowLayout = window.matchMedia("(max-width: 620px)");
function updateNavigationOrientation() {
  document.querySelector(".navigation").setAttribute("aria-orientation", narrowLayout.matches ? "horizontal" : "vertical");
}
narrowLayout.addEventListener("change", updateNavigationOrientation);
updateNavigationOrientation();
// Clicking a switch's description also changes the native checkbox.
for (const control of document.querySelectorAll(".switch")) {
  control.closest(".line-item").querySelector(".label")?.addEventListener("click", (event) => {
    if (event.target.closest("button, a, input, select")) return;
    control.focus();
    control.click();
  });
}

const PRIORITY_NOTES = {
  auto: "Full speed on mains power. Efficiency mode on battery.",
  normal: "Use full speed on mains power and battery.",
  low: "Use a lower priority. Indexing takes longer.",
  eco: "Use Windows efficiency mode to reduce power use. Indexing takes longer.",
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
  $("folder-count").textContent = settings.folders.length;
  renderCode();
  renderPaths($("folder-list"), settings.folders, "Add a folder to start searching your files.", (path) => {
    settings.folders = settings.folders.filter((folder) => folder !== path);
    save();
  });
  renderPaths($("skipped-list"), settings.skipped, "No folders excluded.", (path) => {
    settings.skipped = settings.skipped.filter((folder) => folder !== path);
    save();
  });
  $("skip-types").value = settings.skipTypes.join(", ");
  $("paused").checked = settings.paused;
  $("on-battery").checked = settings.indexOnBattery;
  $("pause-fullscreen").checked = settings.pauseFullscreen;
  $("include-data").checked = Boolean(settings.includeData);
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
    const icon = document.createElement("span");
    icon.className = "folder-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9H3Z"/></svg>';
    const detail = document.createElement("div");
    detail.className = "folder-detail";
    const title = document.createElement("span");
    title.className = "folder-name";
    title.textContent = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
    const name = document.createElement("span");
    name.className = "path";
    name.textContent = path.replaceAll("/", "\\");
    name.title = name.textContent;
    const button = document.createElement("button");
    button.className = "remove";
    button.textContent = "Remove";
    button.setAttribute("aria-label", `Remove folder ${path}`);
    button.onclick = () => remove(path);
    detail.append(title, name);
    line.append(icon, detail, button);
    return line;
  }));
}

let saving = Promise.resolve();
let saveMessageTimer;
let saveRevision = 0;
function showSaveState(state, message) {
  clearTimeout(saveMessageTimer);
  $("saved").textContent = message;
  $("saved").closest(".save-status").dataset.state = state;
}
function save() {
  const revision = ++saveRevision;
  render();
  showSaveState("saving", "Saving…");
  saving = saving.then(async () => {
    try {
      await invoke("save_settings", { settings });
      if (revision === saveRevision) {
        showSaveState("saved", "Changes saved");
        saveMessageTimer = setTimeout(() => showSaveState("saved", "Changes save automatically"), 2500);
      }
    } catch (error) {
      showSaveState("error", String(error));
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
                  "include-data": "includeData",
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

// The model's state as an icon and two lines: a check when it's ready, a spinner while it's checked, a ring that
// fills as it downloads, a warning when the download failed.
const MODEL_ICONS = {
  ready: '<svg viewBox="0 0 32 32"><circle cx="16" cy="16" r="14"/><path d="M10 16.5 L14.2 20.5 L22 12"/></svg>',
  checking: '<svg viewBox="0 0 32 32" class="spin"><circle cx="16" cy="16" r="12" class="track"/><path d="M16 4 A12 12 0 0 1 28 16"/></svg>',
  failed: '<svg viewBox="0 0 32 32"><path d="M16 4 L29 27 H3 Z"/><path d="M16 12 V19 M16 23 V23.2"/></svg>',
};

function downloadRing(percent) {
  const circumference = 2 * Math.PI * 12;
  const filled = circumference * Math.min(100, Math.max(0, percent)) / 100;
  return `<svg viewBox="0 0 32 32"><circle cx="16" cy="16" r="12" class="track"/>`
    + `<circle cx="16" cy="16" r="12" class="fill" stroke-dasharray="${filled} ${circumference}" transform="rotate(-90 16 16)"/>`
    + `<text x="16" y="19.5" text-anchor="middle">${Math.floor(percent)}</text></svg>`;
}

function showModel(model) {
  const megabytes = (bytes) => Math.round((bytes || 0) / 1e6).toLocaleString();
  const percent = model.total > 0 ? (100 * (model.bytes || 0)) / model.total : 0;
  let title = "Model status", text = "";
  switch (model.state) {
    case "ready":
      title = "Ready to search";
      text = "Everything runs on this PC. Nothing you index is uploaded.";
      break;
    case "checking":
      title = "Checking the model";
      text = "Making sure the model files are complete and unchanged…";
      break;
    case "downloading":
      title = "Downloading the model";
      text = model.total && model.bytes ? `${megabytes(model.bytes)} of ${megabytes(model.total)} MB · ${Math.floor(percent)}%`
                                        : "Starting the download…";
      if (model.problem) text += ` · ${model.problem}; trying again`;
      break;
    case "failed": {
      title = "The download didn't finish";
      const reason = String(model.message || "unknown error").replace(/\.$/, "");
      text = `${reason.charAt(0).toUpperCase()}${reason.slice(1)}. Try again, or use files downloaded on another PC.`;
      break;
    }
    default:
      title = "Model unavailable";
  }
  $("model-title").textContent = title;
  $("model").textContent = text;
  const icon = $("model-icon");
  icon.dataset.state = model.state;
  icon.innerHTML = model.state === "downloading" ? downloadRing(percent) : (MODEL_ICONS[model.state] || MODEL_ICONS.checking);
  const stateLabels = { checking: "Checking", ready: "Ready", failed: "Download failed", downloading: "Downloading" };
  $("model-badge").textContent = stateLabels[model.state] || "Unavailable";
  $("model-badge").dataset.state = model.state;
  const progress = $("model-progress");
  progress.hidden = model.state !== "downloading";
  if (model.total > 0) progress.value = Math.min(100, Math.max(0, percent));
  else progress.removeAttribute("value");
  $("model-retry").hidden = model.state !== "failed";
  $("model-files").hidden = model.state !== "failed" && model.state !== "downloading";
}

$("model-retry").addEventListener("click", () => invoke("retry_model").catch(() => {}));
$("model-files").addEventListener("click", () => invoke("use_model_files").catch(() => {}));
$("open-logs").addEventListener("click", () => invoke("open_logs").catch(() => {}));
$("licenses").addEventListener("click", () => invoke("open_acknowledgements").catch((error) => showSaveState("error", String(error))));

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
  $("code-disabled-note").hidden = code.on;
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
    note.className = "path-meta";
    note.textContent = codeEstimates.get(path) ?? "Counting…";
    line?.querySelector(".folder-detail")?.append(note);
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
    showSaveState("error", String(error));
  }
});

start().then(() => {
  loadEditors();
  if (settings.code.on) estimateCode(settings.code.folders);
}).catch((error) => showSaveState("error", `Could not load settings: ${error}`));

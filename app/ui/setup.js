// Trovara's setup window: what to search, the shortcut and fast catch-up, with the model downloading in the
// background from the moment it opens (the app starts that). Folders show what's in them (`trovara-helper estimate`).
// Choose Folders… later opens it straight at the folders.
"use strict";

const tauri = window.__TAURI__;
const invoke = (command, args) => tauri.core.invoke(command, args);
const PAGES = ["welcome", "folders", "shortcut", "catchup", "ready"];

const $ = (id) => document.getElementById(id);
const folders = new Map();   // path → {checked, row, counts}
let page = 0;
let pages = PAGES;
let info = { offered: [], chosen: [], hotkey: "", model: { state: "checking" }, auto: false, setUp: false };

async function start() {
  info = await invoke("setup_info");
  // The usual folders, then any chosen before (Choose Folders… after setup). On a first launch they start checked,
  // all but Downloads (much of what lands there is passing through).
  const chosen = new Set(info.chosen);
  const firstTime = !info.setUp;
  for (const path of info.offered) addFolder(path, firstTime ? !/[\\/]Downloads$/i.test(path) || info.auto : chosen.has(path));
  for (const path of info.chosen) if (!folders.has(path)) addFolder(path, true);
  estimate([...folders.keys()]);
  showModel(info.model);
  tauri.event.listen("model", ({ payload }) => showModel(payload));
  TrovaraHotkey.attach({ keys: $("keys"), change: $("change-key"), reset: $("reset-key"), note: $("key-note"),
                         current: info.hotkey });
  // Choose Folders… after setup: only the folders.
  if (!firstTime) {
    pages = ["folders"];
    document.title = "Choose Folders";
  }
  const asked = pages.indexOf(new URLSearchParams(location.search).get("page"));   // dev snapshots: ?page=folders
  if (asked >= 0) page = asked;
  render();
  if (info.auto) setTimeout(clickThrough, 500);
}

// MARK: Pages

function render() {
  PAGES.forEach((name) => { $(name).hidden = name !== pages[page]; });
  $("back").hidden = page === 0;
  const last = page === pages.length - 1;
  $("next").textContent = last ? (pages.length === 1 ? "Save" : "Start") : "Continue";
  $("next").disabled = pages[page] === "folders" && !chosenFolders().length;
}

$("next").addEventListener("click", async () => {
  if (page < pages.length - 1) {
    page += 1;
    render();
    return;
  }
  $("next").disabled = true;
  try {
    const fast = pages.includes("catchup") && $("fast-catch-up").checked;
    await invoke("finish_setup", { folders: chosenFolders(), fastCatchUp: fast });
  } catch (error) {
    $("model").textContent = String(error);
    $("next").disabled = false;
  }
});

$("back").addEventListener("click", () => {
  page = Math.max(0, page - 1);
  render();
});

/// Debug runs (--debug-setup): every offered folder, every page, then Start.
async function clickThrough() {
  while (page < pages.length - 1) {
    $("next").click();
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  $("next").click();
}

// MARK: Folders

function addFolder(path, checked) {
  const row = document.createElement("label");
  row.className = "folder";
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = checked;
  box.addEventListener("change", () => {
    folders.get(path).checked = box.checked;
    render();
  });
  const text = document.createElement("div");
  text.className = "text";
  const name = document.createElement("div");
  name.className = "name";
  name.textContent = path.split(/[\\/]/).filter(Boolean).pop() || path;
  const where = document.createElement("div");
  where.className = "path";
  where.textContent = path.replaceAll("/", "\\");
  text.append(name, where);
  const counts = document.createElement("div");
  counts.className = "counts";
  counts.textContent = "Counting…";
  row.append(box, text, counts);
  $("folder-list").append(row);
  folders.set(path, { checked, row, counts });
}

function chosenFolders() {
  return [...folders].filter(([, folder]) => folder.checked).map(([path]) => path);
}

async function estimate(paths) {
  if (!paths.length) return;
  let answers = [];
  try {
    answers = await invoke("estimate", { folders: paths });
  } catch (error) {
    for (const path of paths) folders.get(path).counts.textContent = "";
    return;
  }
  for (const path of paths) {
    const answer = answers.find((item) => same(item.folder, path));
    folders.get(path).counts.textContent = answer ? countsText(answer) : "";
  }
}

/// Windows paths compare without case, and either slash.
function same(a, b) {
  const plain = (path) => (path || "").replaceAll("\\", "/").replace(/\/$/, "").toLowerCase();
  return plain(a) === plain(b);
}

/// "1,204 pictures · 37 PDFs · 12 documents · 3 videos · about 6 min"
function countsText(answer) {
  if (answer.readable === false) return "Can't read it";
  const files = answer.files || {};
  const count = (kind) => (typeof files[kind] === "object" ? files[kind]?.count : files[kind]) || 0;
  const part = (n, one, many) => (n ? `${n.toLocaleString()} ${n === 1 ? one : many}` : null);
  const parts = [
    part(count("image") + count("screenshot"), "picture", "pictures"),
    part(count("pdf"), "PDF", "PDFs"),
    part(count("doc"), "document", "documents"),
    part(count("audio"), "recording", "recordings"),
    part(count("video"), "video", "videos"),
  ].filter(Boolean);
  if (!parts.length) return "Nothing Trovara searches in it";
  const seconds = Number(answer.seconds ?? answer.first_pass_seconds ?? 0);
  if (seconds > 0) parts.push(`about ${duration(seconds)}`);
  return parts.join(" · ");
}

function duration(seconds) {
  if (seconds < 90) return `${Math.max(1, Math.round(seconds))} s`;
  if (seconds < 90 * 60) return `${Math.round(seconds / 60)} min`;
  return `${(seconds / 3600).toFixed(1)} h`;
}

$("add-folder").addEventListener("click", async () => {
  const picked = (await invoke("pick_folders")).filter((path) => !folders.has(path));
  for (const path of picked) addFolder(path, true);
  estimate(picked);
  render();
});

// MARK: The model

function showModel(model) {
  const megabytes = (bytes) => Math.round(bytes / 1e6).toLocaleString();
  const lines = {
    checking: "Checking the model…",
    ready: "Model ready",
    failed: `The model couldn't download: ${model.message || "unknown error"}`,
  };
  let text = lines[model.state] || "";
  if (model.state === "downloading") {
    text = model.total && model.bytes
      ? `Downloading the model · ${Math.floor((100 * model.bytes) / model.total)}% of ${megabytes(model.total)} MB`
      : "Downloading the model…";
    if (model.problem) text += ` (${model.problem}; trying again)`;
  }
  $("model").textContent = text;
  $("model-retry").hidden = model.state !== "failed";
  $("model-files").hidden = model.state !== "failed" && model.state !== "downloading";
}

$("model-retry").addEventListener("click", () => invoke("retry_model").catch((error) => { $("model").textContent = String(error); }));
$("model-files").addEventListener("click", () => invoke("use_model_files").catch((error) => { $("model").textContent = String(error); }));

start();

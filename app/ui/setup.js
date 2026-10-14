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
let renderedPage = null;
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
  document.body.classList.toggle("folder-mode", pages[page] === "folders");
  if (renderedPage !== pages[page]) {
    document.querySelector("main").scrollTop = 0;
    renderedPage = pages[page];
  }
  PAGES.forEach((name) => { $(name).hidden = name !== pages[page]; });
  $("back").hidden = page === 0;
  const last = page === pages.length - 1;
  $("next").textContent = last ? (pages.length === 1 ? "Save" : "Start") : "Continue";
  const count = chosenFolders().length;
  $("next").disabled = pages[page] === "folders" && !count;
  $("folder-selection").textContent = count ? `${count} ${count === 1 ? "folder" : "folders"} selected` : "Select at least one folder";
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
  const icon = document.createElement("span");
  icon.className = "folder-icon";
  icon.setAttribute("aria-hidden", "true");
  const drive = /^[a-z]:[\\/]?$/i.test(path);
  icon.innerHTML = drive
    ? '<svg viewBox="0 0 24 24"><path d="M5 4 H19 L21 15 V20 H3 V15 Z M3 15 H21 M16 17.5 H18"/><circle cx="6" cy="17.5" r=".5"/></svg>'
    : '<svg viewBox="0 0 24 24"><path d="M3 7 V19 H21 V7 H11 L9 4 H3 Z"/></svg>';
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = checked;
  box.setAttribute("aria-label", `Search ${path.replaceAll("/", "\\")}`);
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
  where.title = where.textContent;
  const counts = document.createElement("div");
  counts.className = "counts";
  counts.textContent = "Counting…";
  text.append(name, where, counts);
  row.append(icon, text, box);
  $("folder-list").append(row);
  folders.set(path, { checked, row, counts });
}

function chosenFolders() {
  return [...folders].filter(([, folder]) => folder.checked).map(([path]) => path);
}

/// What each folder holds: the counts Trovara has (at once), and a count only for folders it hasn't counted, or whose
/// files changed since; those keep showing the last counts while they're counted again.
async function estimate(paths) {
  if (!paths.length) return;
  let known = [];
  try {
    known = await invoke("estimate", { folders: paths, cached: true });
  } catch (error) {}
  const wanted = [];
  for (const path of paths) {
    const answer = known.find((item) => same(item.folder, path));
    if (answer) folders.get(path).counts.textContent = countsText(answer);
    if (!answer || answer.stale) wanted.push(path);
  }
  if (!wanted.length) return;
  let answers = [];
  try {
    answers = await invoke("estimate", { folders: wanted });
  } catch (error) {
    for (const path of wanted) if (!known.some((item) => same(item.folder, path))) folders.get(path).counts.textContent = "";
    return;
  }
  for (const path of wanted) {
    const answer = answers.find((item) => same(item.folder, path));
    if (answer) folders.get(path).counts.textContent = countsText(answer);
    else if (!known.some((item) => same(item.folder, path))) folders.get(path).counts.textContent = "";
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

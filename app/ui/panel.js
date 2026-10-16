// Trovara's search panel (the hotkey) and search window (Ctrl+O, search.html): every keystroke shows keyword results
// at once (the last word as a prefix); once typing pauses for 120 ms the meaning results replace them (the query was
// embedded while you typed). Results far weaker than the best are folded under "N weaker matches". The helper
// (`trovara-helper search-service`) does the work. The window adds filters by kind and shows pictures in a grid.
"use strict";

const tauri = window.__TAURI__;
const invoke = (command, args) => tauri.core.invoke(command, args);
const ask = (request) => invoke("ask", { request });
const fileURL = (path) => tauri.core.convertFileSrc(path);

const TYPING_PAUSE = 120;          // ms after the last keystroke until the meaning results show
const ROWS_PER_GROUP = 6;
const FIELD_HEIGHT = 56;
const PANEL_HEIGHT = 10000;     // "as tall as results may be": the app sizes it for the screen (up to 640)
const WEAKER = "\u0000weaker";     // the selectable row that folds and unfolds the weaker matches
const WINDOW = document.body.classList.contains("window");   // the search window, not the panel
const FILE_MANAGER = "Explorer";
// Pictures every web view shows as they are; anything else (HEIC, TIFF, RAW…) comes as JPEG from the helper.
const SHOWN_AS_IS = new Set(["jpg", "jpeg", "png", "gif", "webp", "bmp"]);
const PICTURES = new Set(["image", "screenshot"]);
const GROUP_OF = { screenshot: "Screenshots", image: "Images", pdf: "Documents", doc: "Documents", video: "Video",
                   audio: "Audio", code: "Code" };
const KIND_TITLE = { screenshot: "Screenshot", image: "Image", pdf: "PDF", doc: "Document", audio: "Audio",
                     video: "Video", code: "Code" };
// The window's filters: [id, label, kinds (null: all), code search].
const FILTERS = [
  ["all", "All", null], ["pictures", "Pictures", ["image", "screenshot"]], ["screenshots", "Screenshots", ["screenshot"]],
  ["documents", "Documents", ["pdf", "doc"]], ["video", "Video", ["video"]], ["audio", "Audio", ["audio"]],
  ["code", "Code", null, true],
];

const $ = (id) => document.getElementById(id);
const $query = $("query"), $body = $("body"), $list = $("list"), $preview = $("preview");
const $footer = $("footer"), $summary = $("summary"), $keys = $("keys"), $activity = $("activity");
const $chips = $("chips");

const state = {
  generation: 0,
  finalGeneration: -1,   // the generation whose meaning results are showing: late keyword results don't replace them
  timer: null,
  phase: "empty",        // empty | words | meaning | wordsOnly | codeHint
  text: "",              // what was searched (without "code:")
  isCode: false,
  strong: [],
  weaker: [],
  groups: [],
  showWeaker: false,
  selection: null,       // a result's path, or WEAKER
  navigated: false,      // the arrow keys moved the selection since the last edit
  encoder: "stopped",
  height: FIELD_HEIGHT,
  home: "",
  error: null,
  indexing: null,        // {done, total} while the index helper works through new files
  filter: "all",         // the window's filter
  grid: false,           // the window shows pictures in a grid
  codeMatches: 0,        // the words also match this many stretches of code (a hint row, Tab to search code)
};
let debugPending = false;

// MARK: Searching

function queryChanged() {
  const generation = ++state.generation;
  clearTimeout(state.timer);
  state.navigated = false;
  state.error = null;
  closeViewer();
  const typed = $query.value.trim();
  const filter = FILTERS.find(([id]) => id === state.filter) ?? FILTERS[0];
  const prefixed = /^code:/i.test(typed);
  const code = prefixed || filter[3] === true;
  const text = prefixed ? typed.slice(5).trim() : typed;
  const kinds = code ? null : filter[2];
  const limit = WINDOW ? 120 : 40;
  state.isCode = code;
  state.codeMatches = 0;
  if (!typed || (code && !text)) {
    show({ hits: [], strong: 0 }, text, typed || (code && WINDOW) ? "codeHint" : "empty");
    return;
  }
  // Words now. The last word is probably half-typed, so it matches as a prefix.
  ask({ op: "search", q: text, code, kinds, meaning: false, prefix_last: true, limit })
    .then((reply) => {
      if (generation === state.generation && state.finalGeneration !== generation) show(reply, text, "words");
    })
    .catch(failed);
  // The meaning is embedded now, so its vector is ready when typing pauses: the pause is the whole wait.
  ask({ op: "embed_ahead", q: text, code }).catch(() => {});
  state.timer = setTimeout(() => {
    ask({ op: "search", q: text, code, kinds, meaning: true, limit })
      .then((reply) => {
        if (generation !== state.generation) return;
        state.finalGeneration = generation;
        // Meaning still warming up (the panel just opened after a while): words for now, meaning once it's ready.
        if (reply.warming) state.encoder = "starting";
        show(reply, text, reply.meaning ? "meaning" : reply.warming ? "words" : "wordsOnly");
        if (debugPending) report();
      })
      .catch(failed);
    // Code never mixes into these results, but when the words are in code too, a row says so (PRD F-COD-11).
    if (!code) {
      ask({ op: "search", q: text, code: true, meaning: false, limit: 5 })
        .then((reply) => {
          if (generation !== state.generation || !reply.hits?.length) return;
          state.codeMatches = reply.hits.length;
          renderList();
        })
        .catch(() => {});
    }
  }, TYPING_PAUSE);
}

function show(reply, text, phase) {
  const hits = reply.hits || [];
  const strong = Math.min(reply.strong ?? hits.length, hits.length);
  // A word that's in no file was searched as the closest one that is ("beyound" → "beyond"): previews mark that.
  state.corrected = reply.corrected || null;
  state.text = state.corrected || text;
  state.phase = phase;
  state.strong = hits.slice(0, strong);
  state.weaker = hits.slice(strong);
  // The window shows pictures in a grid: filtered to pictures, or when every strong result is one.
  state.grid = WINDOW && !state.isCode && state.strong.length > 0
    && (state.filter === "pictures" || state.filter === "screenshots" || state.strong.every((hit) => PICTURES.has(hit.kind)));
  document.body.classList.toggle("grid-view", state.grid);
  // Grouped by kind, the best hit's group first; code is one group with the room of all of them.
  const groups = new Map();
  for (const hit of state.strong) {
    const title = GROUP_OF[hit.kind] || "Files";
    if (!groups.has(title)) groups.set(title, []);
    groups.get(title).push(hit);
  }
  const perGroup = WINDOW ? 1000 : state.isCode ? ROWS_PER_GROUP * 3 : ROWS_PER_GROUP;
  state.groups = state.grid
    ? [{ title: null, rows: state.strong, total: state.strong.length }]
    : [...groups].map(([title, rows]) => ({ title, rows: rows.slice(0, perGroup), total: rows.length }));
  if (!state.navigated) state.showWeaker = false;
  // Follow the top hit until you pick something with the arrow keys.
  const ids = rowIDs();
  if (!state.navigated || !ids.includes(state.selection)) state.selection = ids[0] ?? null;
  render();
}

function failed(error) {
  state.error = String(error);
  renderFooter();
}

function rowIDs() {
  return [
    ...state.groups.flatMap((group) => group.rows.map((hit) => hit.path)),
    ...(state.weaker.length ? [WEAKER] : []),
    ...(state.showWeaker ? state.weaker.map((hit) => hit.path) : []),
  ];
}

function selectedHit() {
  return [...state.strong, ...state.weaker].find((hit) => hit.path === state.selection) ?? null;
}

function move(step) {
  const ids = rowIDs();
  if (!ids.length) return;
  const index = ids.indexOf(state.selection);
  state.selection = ids[Math.max(0, Math.min(ids.length - 1, (index < 0 ? -1 : index) + step))];
  state.navigated = true;
  render();
}

function toggleWeaker() {
  state.showWeaker = !state.showWeaker;
  state.selection = WEAKER;
  render();
}

/// Opens the file; PDFs at the page and recordings at the moment that matched, when its app can be told where.
function openSelected() {
  const hit = selectedHit();
  if (!hit) return;
  if (hit.kind === "code") {
    // In the code editor, at the line that matched (a name's definition), else the stretch's first line.
    const line = hit.focus ?? (hit.segment === "lines" ? hit.loc : null);
    invoke("open_code", { path: hit.path, line }).catch(failed);
    return;
  }
  const at = ["page", "page_image", "frame", "audio", "subtitle"].includes(hit.segment) && hit.loc != null;
  invoke("open_path", { path: hit.path, kind: at ? hit.kind : null, loc: at ? hit.loc : null }).catch(failed);
}

function revealSelected() {
  const hit = selectedHit();
  if (hit) invoke("reveal", { path: hit.path }).catch(failed);
}

/// Ctrl+C: the file itself, to paste in Explorer, a mail or a chat. Ctrl+Shift+C: its path, as text.
function copySelected(asPath) {
  const hit = selectedHit();
  if (!hit) return;
  if (asPath) {
    navigator.clipboard.writeText(hit.path).then(() => toast("Path copied")).catch(failed);
  } else {
    invoke("copy_files", { paths: [hit.path] }).then(() => toast(`Copied ${nameOf(hit.path)}`)).catch(failed);
  }
}

function toast(text) {
  let note = document.querySelector(".toast");
  if (!note) {
    note = element("div", "toast");
    document.body.append(note);
  }
  note.textContent = text;
  note.style.opacity = "1";
  clearTimeout(note.timer);
  note.timer = setTimeout(() => { note.style.opacity = "0"; }, 1400);
}

// MARK: Drawing

function render() {
  const empty = state.phase === "empty";
  const height = empty ? FIELD_HEIGHT : PANEL_HEIGHT;
  if (!WINDOW && height !== state.height) {
    state.height = height;
    invoke("panel_height", { height }).catch(() => {});
  }
  $body.hidden = empty && !WINDOW;
  $footer.hidden = empty && !WINDOW;
  if (empty && !WINDOW) return;
  renderList();
  renderPreview();
  renderFooter();
}

function renderList() {
  $list.replaceChildren();
  $list.classList.toggle("grid", state.grid);
  const nothing = !state.strong.length && !state.weaker.length && !state.codeMatches;
  $list.hidden = nothing;
  if (nothing) return;
  if (state.corrected) {
    const note = element("div", "spelled");
    note.append("Showing results for ", element("b", "", state.corrected));
    $list.append(note);
  }
  for (const group of state.groups) {
    if (group.title) $list.append(element("div", "group-title", group.title));
    for (const hit of group.rows) $list.append(state.grid ? cell(hit) : row(hit));
  }
  if (state.weaker.length) {
    const fold = element("div", "weaker" + (state.showWeaker ? " open" : "") + (state.selection === WEAKER ? " selected" : ""));
    fold.innerHTML = ICONS.chevron;
    const count = state.weaker.length;
    fold.append(state.showWeaker ? "Weaker matches" : `${count} weaker ${count === 1 ? "match" : "matches"}`);
    fold.onclick = toggleWeaker;
    $list.append(fold);
    if (state.showWeaker) for (const hit of state.weaker) $list.append(state.grid ? cell(hit) : row(hit));
  }
  if (state.codeMatches && !state.isCode) {
    const hint = element("div", "code-hint");
    hint.innerHTML = ICONS.code;
    const count = state.codeMatches >= 5 ? "5+" : String(state.codeMatches);
    hint.append(element("span", "", `Also ${count} ${state.codeMatches === 1 ? "match" : "matches"} in your code`),
                element("kbd", "", "Tab"));
    hint.onclick = searchCode;
    $list.append(hint);
  }
  $list.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
}

function row(hit) {
  const item = element("div", "row" + (state.selection === hit.path ? " selected" : ""));
  item.setAttribute("role", "option");
  item.append(thumbnail(hit));
  const text = element("div", "row-text");
  const title = element("div", "row-title");
  title.append(element("span", "name", nameOf(hit.path)));
  const place = placeOf(hit);
  if (place) title.append(element("span", "badge", place));
  const sub = element("div", "row-sub");
  // The words found, where keyword search found them; else where the file is.
  if (hit.excerpt && hit.excerpt.includes("«")) sub.innerHTML = marked(hit.excerpt);
  else sub.textContent = folderOf(hit);
  text.append(title, sub);
  item.append(text);
  item.onclick = () => select(hit);
  item.ondblclick = openSelected;
  return item;
}

/// A tile in the window's grid: the picture, its name under it.
function cell(hit) {
  const item = element("div", "cell" + (state.selection === hit.path ? " selected" : ""));
  item.setAttribute("role", "option");
  item.title = hit.path;
  item.append(thumbnail(hit, 160), element("div", "name", nameOf(hit.path)));
  item.onclick = () => select(hit);
  item.ondblclick = openSelected;
  return item;
}

function select(hit) {
  state.selection = hit.path;
  state.navigated = true;
  render();
}

/// A picture of the file for its row: the picture itself, a video's frame at the moment, a PDF's page; else its
/// kind's icon. All come small from the helper (a full-size photo per row would cost the web view hundreds of MB).
function thumbnail(hit, size = 36) {
  const pixels = Math.round(size * (window.devicePixelRatio || 1));
  const what = PICTURES.has(hit.kind) ? ["image", null]
    : hit.kind === "video" ? ["video", hit.loc ?? 0.5]
    : hit.kind === "pdf" ? ["pdf", hit.segment === "page" || hit.segment === "page_image" ? hit.loc : 1]
    : null;
  if (!what) return kindIcon(hit.kind);
  const image = element("img", "thumb");
  image.alt = "";
  image.loading = "lazy";
  rendered(hit.path, what[0], what[1], pixels)
    .then((path) => { image.src = fileURL(path); })
    .catch(() => image.replaceWith(kindIcon(hit.kind)));
  return image;
}

function renderPreview() {
  $preview.replaceChildren();
  if (!state.strong.length && !state.weaker.length) {
    if (state.phase === "empty") {
      $preview.append(message("Search everything you've kept", "Describe it the way you remember it: what's in a "
        + "picture, a moment in a video, a passage in a PDF."));
    } else if (state.phase === "codeHint") {
      $preview.append(message("Search your code", "Say what the code does, or type a name that's in it."));
    } else if (state.indexing) {
      const { done, total } = state.indexing;
      $preview.append(message("Nothing yet", `Still indexing (${done} of ${total} files). Until a file is read, `
        + "Trovara finds it only by its name."));
    } else if (state.isCode) {
      $preview.append(message("No good matches in your code", "Try saying what the code does, or a name that's in it."));
    } else {
      $preview.append(message("No good matches", "Try what you'd see or hear in it, or words you remember from it."));
    }
    return;
  }
  if (state.selection === WEAKER) {
    const count = state.weaker.length;
    $preview.append(message(state.showWeaker ? "Weaker matches" : `${count} weaker ${count === 1 ? "match" : "matches"}`,
      "They match much less well than the results above. Press Enter to " + (state.showWeaker ? "fold them away." : "show them.")));
    return;
  }
  const hit = selectedHit();
  if (!hit) return;
  const shown = picture(hit);
  if (canView(hit)) {
    // Space, a click on the picture, or this: the file large over the panel (the viewer).
    const expand = element("button", "expand");
    expand.type = "button";
    expand.title = "View large (Space)";
    expand.setAttribute("aria-keyshortcuts", "Space");
    expand.innerHTML = ICONS.expand;
    expand.onclick = (event) => { event.stopPropagation(); openViewer(hit); };
    shown.append(expand);
  }
  $preview.append(shown, facts(hit));
}

/// The big picture: the photo or screenshot, the frame at the moment (it plays from there), the page, what's heard
/// at the moment, or the passage that matched.
function picture(hit) {
  const frame = element("div", "picture");
  const image = element("img");
  image.alt = "";
  frame.append(image);
  const big = WINDOW ? 1600 : 1024;
  switch (hit.kind) {
    case "image":
    case "screenshot": {
      // A screenshot whose words matched: the helper marks them yellow where OCR reads them (F-PRE-10).
      const marking = hit.kind === "screenshot" && hit.found?.length ? { q: state.text, words: hit.found } : null;
      frame.onclick = () => openViewer(hit);
      if (SHOWN_AS_IS.has(extensionOf(hit.path)) && !marking) image.src = fileURL(hit.path);
      else {
        rendered(hit.path, "image", null, big, marking ?? undefined)
          .then((path) => { image.src = fileURL(path); })
          .catch(() => { if (SHOWN_AS_IS.has(extensionOf(hit.path))) image.src = fileURL(hit.path); });
      }
      return frame;
    }
    case "pdf":
      // The page with the found words marked yellow, zoomed to the part around them in the preview's shape
      // (F-PRE-04); a click shows the whole page large.
      frame.onclick = () => openViewer(hit);
      rendered(hit.path, "pdf", hit.loc ?? 1, big, { q: state.text, words: hit.found ?? [], aspect: previewAspect() })
        .then((path) => { image.src = fileURL(path); })
        .catch(() => {});
      return frame;
    case "video": {
      // The frame at the moment, its time on it; a click plays the video from there.
      const at = hit.loc ?? 0;
      rendered(hit.path, "video", at, big).then((path) => { image.src = fileURL(path); }).catch(() => {});
      const moment = element("div", "moment");
      const play = element("div", "play");
      play.innerHTML = ICONS.play;
      moment.append(image, play, element("span", "at", clock(at)));
      frame.replaceChildren(moment);
      frame.onclick = () => {
        const video = element("video");
        video.src = `${fileURL(hit.path)}#t=${at}`;
        video.controls = true;
        video.autoplay = true;
        // A format the web view can't play (MKV, AVI…): the file opens in its own app at the moment instead.
        video.onerror = () => openSelected();
        frame.replaceChildren(video);
      };
      if (hit.segment !== "subtitle") return frame;
      // What's said there, under the frame, the words marked (PRD F-RD-13).
      const said = element("div", "passage said");
      said.dir = "auto";
      said.textContent = hit.excerpt?.replace(/[«»]/g, "") ?? "";
      ask({ op: "text", segment: hit.segment_id, q: state.text, words: hit.found ?? [] })
        .then((reply) => { if (reply.text) TrovaraPreview.passage(said, reply); })
        .catch(() => {});
      const both = element("div", "picture with-said");
      both.append(frame, said);
      return both;
    }
    case "audio": {
      const audio = element("div", "audio-picture");
      audio.innerHTML = ICONS.audio;
      const player = element("audio");
      player.src = `${fileURL(hit.path)}#t=${hit.loc ?? 0}`;
      player.controls = true;
      player.preload = "metadata";
      audio.append(player);
      frame.replaceChildren(audio);
      return frame;
    }
    default:
      return textPreview(hit);
  }
}

/// The part of a file that matched, its words marked yellow and the preview opened at the first (PRD §6.1b):
/// Markdown formatted, code coloured with its line numbers, anything else as a passage on a page-like card.
function textPreview(hit) {
  const markdown = TrovaraPreview.isMarkdown(hit.path);
  const lines = hit.kind === "code" && hit.segment === "lines" && hit.loc != null;
  const passage = element("div", "passage" + (markdown ? " markdown" : lines ? " code" : ""));
  passage.dir = "auto"; // Arabic and Hebrew read from the right (F-PRE-08)
  passage.textContent = (hit.excerpt ?? "").replace(/[«»]/g, "");
  const part = partOf(hit);
  const box = element("div", "passage-box");
  box.append(passage);
  if (part) box.append(element("span", "part", part));
  ask({ op: "text", segment: hit.segment_id, code: state.isCode, q: state.text, words: hit.found ?? [] })
    .then((reply) => {
      if (!reply.text) return;
      if (markdown) TrovaraPreview.markdown(passage, reply);
      else if (lines) TrovaraPreview.code(passage, reply, hit.path, Math.trunc(hit.loc), hit.focus);
      else TrovaraPreview.passage(passage, reply);
    })
    .catch(() => {});
  return box;
}

/// The shape (width / height) the preview's picture has: the room left above the file's facts.
function previewAspect() {
  const width = $preview.clientWidth - 32, height = $preview.clientHeight - 30 - 150;
  return width > 0 && height > 0 ? Math.round((width / height) * 20) / 20 : 1.5;
}

/// "Part 2 of 5", when a long document matched in one of its parts.
function partOf(hit) {
  const parts = Number(hit.info?.chunks);
  return hit.segment === "chunk" && hit.loc != null && parts > 1 ? `Part ${Math.trunc(hit.loc)} of ${parts}` : null;
}

function facts(hit) {
  const box = element("div", "facts");
  box.append(element("div", "title", nameOf(hit.path)), element("div", "line", folderOf(hit)),
             element("div", "line", details(hit)));
  const reason = element("div", "reason");
  reason.innerHTML = ICONS.found;
  reason.append(element("span", "", reasonOf(hit)));
  box.append(reason);
  if (hit.excerpt && hit.excerpt.includes("«") && hit.kind !== "doc" && hit.kind !== "code") {
    const words = element("div", "reason-text");
    words.innerHTML = marked(hit.excerpt);
    box.append(words);
  }
  const buttons = element("div", "buttons");
  const open = element("button", "primary", hit.kind === "code" ? `Open in ${codeEditor()}` : "Open");
  open.type = "button";
  open.title = "Open the file (Enter)";
  open.setAttribute("aria-keyshortcuts", "Enter");
  open.onclick = openSelected;
  const reveal = element("button", "", `Show in ${FILE_MANAGER}`);
  reveal.type = "button";
  reveal.title = `Show the file in ${FILE_MANAGER} (Ctrl+Enter)`;
  reveal.setAttribute("aria-keyshortcuts", "Control+Enter");
  reveal.onclick = revealSelected;
  const copy = element("button", "", "Copy");
  copy.type = "button";
  copy.setAttribute("aria-keyshortcuts", "Control+C");
  copy.title = "Ctrl+C: the file, to paste anywhere";
  copy.onclick = () => copySelected(false);
  buttons.append(open, reveal, copy);
  box.append(buttons);
  return box;
}

function renderFooter() {
  const strong = state.strong.length, weaker = state.weaker.length;
  let summary;
  if (state.error) summary = `Trovara's helper: ${state.error}`;
  else if (state.phase === "empty") summary = "Type to search";
  else if (state.phase === "words") {
    summary = state.encoder === "starting" ? "Matching words; meaning search is warming up…" : "Matching words…";
  } else if (state.phase === "wordsOnly") summary = "Words only: meaning search isn't available";
  else if (state.phase === "codeHint") summary = "Searching your code";
  else summary = `${strong} ${strong === 1 ? "result" : "results"}` + (weaker ? ` · ${weaker} weaker` : "");
  // Results while indexing are partial: say so where they're counted.
  if (state.indexing && !state.error) summary += ` · still indexing (${state.indexing.done} of ${state.indexing.total})`;
  $summary.textContent = summary;
  $keys.textContent = WINDOW ? `↩ Open   Space View   Ctrl+↩ Show in ${FILE_MANAGER}   Ctrl+C Copy`
                             : "↩ Open   Space View   Ctrl+↩ Show   Ctrl+C Copy   Ctrl+O Window";
}

function message(title, text) {
  const box = element("div", "message");
  box.append(element("strong", "", title), element("span", "", text));
  return box;
}

// MARK: What a result says

/// The page or moment that matched, as a badge: "p. 3", "0:06", "♪ 0:25" (a video's sound), "L12–40" (code).
function placeOf(hit) {
  if (hit.loc == null) return null;
  switch (hit.segment) {
    case "page":
    case "page_image": return `p. ${Math.trunc(hit.loc)}`;
    case "frame": return clock(hit.loc);
    case "audio": return hit.kind === "video" ? `♪ ${clock(hit.loc)}` : clock(hit.loc);
    case "subtitle": return `“ ${clock(hit.loc)}`;
    case "lines": {
      const first = Math.trunc(hit.loc), last = Math.trunc(hit.loc_end ?? hit.loc);
      return first === last ? `L${first}` : `L${first}–${last}`;
    }
    case "cells": return span(hit);
    default: return null;
  }
}

function span(hit) {
  if (hit.loc == null) return null;
  const first = Math.trunc(hit.loc), last = Math.trunc(hit.loc_end ?? hit.loc);
  const what = hit.segment === "cells" ? ["cell", "cells"] : ["line", "lines"];
  return first === last ? `${what[0]} ${first}` : `${what[1]} ${first}–${last}`;
}

/// Why it's here, in a few words: "Matches the picture", "Matches the frame at 0:06".
function reasonOf(hit) {
  const meaning = (hit.z ?? 0) >= 2, words = hit.keyword_rank != null;
  if (hit.segment === "subtitle" && hit.loc != null) {
    return meaning ? `Matches what's said at ${clock(hit.loc)}` : `Says your words at ${clock(hit.loc)}`;
  }
  if (!meaning) return words ? "Has your words" : "Matches its name";
  const at = hit.loc != null ? clock(hit.loc) : "";
  const page = hit.loc != null ? `page ${Math.trunc(hit.loc)}` : "a page";
  switch (hit.segment) {
    case "image": return hit.kind === "screenshot" ? "Matches what's on screen" : "Matches the picture";
    case "ocr": return "Matches the text in it";
    case "frame": return `Matches the frame at ${at}`;
    case "audio": return hit.kind === "video" ? `Matches the sound at ${at}` : `Matches what's heard at ${at}`;
    case "page": return `Matches the text on ${page}`;
    case "page_image": return `Matches ${page}`;
    case "chunk": return "Matches its text";
    case "lines":
    case "cells": return `Matches ${span(hit) ?? "the code"}`;
    default: return "Matches its name";
  }
}

/// "Screenshot · Oct 7, 2026 · 1440 × 900 · 62 KB", "PDF · … · 463 pages, 120 read so far · 23 MB"
function details(hit) {
  const info = hit.info || {};
  const parts = [KIND_TITLE[hit.kind] || "File", dateOf(hit.modified)];
  if (info.width && info.height) parts.push(`${info.width} × ${info.height}`);
  const readTo = info.read_to != null ? Number(info.read_to) : null;
  if (info.pages) {
    const pages = Number(info.pages);
    parts.push((pages === 1 ? "1 page" : `${pages.toLocaleString()} pages`) + (readTo != null ? `, ${readTo} read so far` : ""));
  } else if (readTo != null) parts.push("partly read so far");
  if (info.lines) parts.push(Number(info.lines) === 1 ? "1 line" : `${Number(info.lines).toLocaleString()} lines`);
  if (info.duration) parts.push(clock(Number(info.duration)));
  if (hit.size > 0) parts.push(bytes(hit.size));
  return parts.join(" · ");
}

function nameOf(path) {
  return path.split(/[\\/]/).pop();
}

function extensionOf(path) {
  const name = nameOf(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/// Where the file is: "~\Downloads\Screenshots"; code says where it is in its repo: "billing\payments".
function folderOf(hit) {
  const folder = hit.path.slice(0, Math.max(0, hit.path.search(/[\\/][^\\/]*$/)));
  const repo = hit.info?.repo;
  const inRepo = repo && hit.path.toLowerCase().replaceAll("/", "\\").startsWith(repo.toLowerCase().replaceAll("/", "\\") + "\\");
  if (hit.kind === "code" && inRepo) {
    // From the repo's own folder: "billing-service\billing".
    return folder.slice(repo.slice(0, repo.search(/[\\/][^\\/]*$/)).length + 1).replaceAll("/", "\\");
  }
  const home = state.home.toLowerCase().replaceAll("/", "\\");
  const shown = folder.replaceAll("/", "\\");
  return home && shown.toLowerCase().startsWith(home) ? "~" + shown.slice(home.length) : shown;
}

function clock(seconds) {
  const total = Math.trunc(seconds);
  const two = (n) => String(n).padStart(2, "0");
  return total >= 3600 ? `${Math.trunc(total / 3600)}:${two(Math.trunc(total / 60) % 60)}:${two(total % 60)}`
                       : `${Math.trunc(total / 60)}:${two(total % 60)}`;
}

function dateOf(seconds) {
  return new Date(seconds * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/// Sizes the way Explorer's neighbours count them (1 KB = 1,000 bytes): "62 KB", "2.1 MB".
function bytes(count) {
  if (count < 1000) return `${count} bytes`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = count / 1000, unit = 0;
  while (value >= 1000 && unit < units.length - 1) { value /= 1000; unit += 1; }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/// An excerpt with the words keyword search found (marked « ») in bold.
function marked(excerpt) {
  return escaped(excerpt).replace(/«([^»]*)»/g, "<b>$1</b>");
}

function escaped(text) {
  return text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function kindIcon(kind) {
  const icon = element("div", "thumb icon");
  icon.innerHTML = ICONS[kind] || ICONS.doc;
  return icon;
}

/// The editor code opens in ("VS Code"), asked for once a code result shows; the preview redraws when it's in.
let editorName = null;
function codeEditor() {
  if (editorName === null) {
    editorName = "Editor";
    invoke("code_editors").then((info) => {
      editorName = info.opensIn === "its app" ? "Editor" : info.opensIn;
      renderPreview();
    }).catch(() => {});
  }
  return editorName;
}

/// The helper's JPEG of a page, a frame or a picture, asked for once per file and place (and, for a marked PDF
/// page, per query: `marking` is {q, words}).
const renders = new Map();
function rendered(path, kind, loc, pixels, marking) {
  const key = `${path}|${kind}|${loc}|${pixels}|${marking ? JSON.stringify(marking) : ""}`;
  if (!renders.has(key)) renders.set(key, ask({ op: "render", path, kind, loc, pixels, ...(marking ?? {}) }).then((reply) => reply.image));
  return renders.get(key);
}

const ICONS = {
  chevron: '<svg class="chevron" viewBox="0 0 16 16" width="12" height="12"><path d="M6 3.5 L10.5 8 L6 12.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  play: '<svg viewBox="0 0 20 20"><path d="M5 3 L17 10 L5 17 Z"/></svg>',
  found: '<svg viewBox="0 0 24 24"><circle cx="10" cy="10" r="6.5"/><path d="M15 15 L20.5 20.5 M10 7 V13 M7 10 H13"/></svg>',
  doc: '<svg viewBox="0 0 24 24"><path d="M6 3 H14 L19 8 V21 H6 Z M14 3 V8 H19 M9 12 H16 M9 15 H16 M9 18 H13"/></svg>',
  pdf: '<svg viewBox="0 0 24 24"><path d="M6 3 H14 L19 8 V21 H6 Z M14 3 V8 H19 M9 13 H16 M9 16 H14"/></svg>',
  code: '<svg viewBox="0 0 24 24"><path d="M9 7 L4 12 L9 17 M15 7 L20 12 L15 17"/></svg>',
  audio: '<svg viewBox="0 0 24 24"><path d="M3 12 H5 M7 8 V16 M10 5 V19 M13 9 V15 M16 6 V18 M19 10 V14 M21 12 H22"/></svg>',
  video: '<svg viewBox="0 0 24 24"><path d="M4 6 H15 V18 H4 Z M15 10 L20 7 V17 L15 14"/></svg>',
  image: '<svg viewBox="0 0 24 24"><path d="M4 5 H20 V19 H4 Z M4 16 L9 11 L13 15 L16 12 L20 16"/></svg>',
  expand: '<svg viewBox="0 0 24 24"><path d="M14 4 H20 V10 M20 4 L13.5 10.5 M10 20 H4 V14 M4 20 L10.5 13.5"/></svg>',
  close: '<svg viewBox="0 0 24 24"><path d="M6 6 L18 18 M18 6 L6 18"/></svg>',
  back: '<svg viewBox="0 0 24 24"><path d="M15 5 L8 12 L15 19"/></svg>',
  next: '<svg viewBox="0 0 24 24"><path d="M9 5 L16 12 L9 19"/></svg>',
};
ICONS.screenshot = ICONS.image;

// MARK: The viewer (PRD F-PRE-11): the file large over the panel or window, like Quick Look

/// What's open in the viewer: {hit, page, zoom} (page: a PDF's page shown), or null.
let viewing = null;
const $viewer = element("div", "viewer");
$viewer.hidden = true;
$viewer.setAttribute("role", "dialog");
$viewer.setAttribute("aria-label", "Viewer");
document.body.append($viewer);

function canView(hit) {
  return hit.kind !== "audio" && hit.kind !== "video";
}

function openViewer(hit) {
  if (!hit || !canView(hit)) return;
  viewing = { hit, page: Math.max(1, Math.trunc(hit.loc ?? 1)), zoom: 1 };
  renderViewer();
}

function closeViewer() {
  if (!viewing) return;
  viewing = null;
  $viewer.hidden = true;
  $viewer.replaceChildren();
}

function renderViewer() {
  const { hit } = viewing;
  const pages = Number(hit.info?.pages) || null;
  const bar = element("div", "viewer-bar");
  const title = element("div", "viewer-title", nameOf(hit.path));
  bar.append(title);
  if (hit.kind === "pdf") {
    const back = viewerButton(ICONS.back, "Previous page (←)", () => turnPage(-1));
    const next = viewerButton(ICONS.next, "Next page (→)", () => turnPage(1));
    back.disabled = viewing.page <= 1;
    next.disabled = pages != null && viewing.page >= pages;
    bar.append(back, element("span", "viewer-page", pages ? `Page ${viewing.page} of ${pages}` : `Page ${viewing.page}`), next);
  }
  const zoom = element("span", "viewer-zoom", `${Math.round(viewing.zoom * 100)}%`);
  bar.append(viewerButton("−", "Smaller (−)", () => zoomBy(1 / 1.25)), zoom, viewerButton("+", "Larger (+)", () => zoomBy(1.25)),
             viewerButton(ICONS.close, "Close (Esc)", closeViewer));
  const stage = element("div", "viewer-stage");
  stage.style.setProperty("--zoom", viewing.zoom);
  stage.onclick = (event) => { if (event.target === stage) closeViewer(); };
  stage.addEventListener("wheel", (event) => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    zoomBy(event.deltaY < 0 ? 1.1 : 1 / 1.1);
  }, { passive: false });
  if (PICTURES.has(hit.kind) || hit.kind === "pdf") {
    const image = element("img", "viewer-image" + (hit.kind === "pdf" ? " page" : ""));
    image.alt = "";
    const marking = hit.found?.length && (hit.kind === "pdf" || hit.kind === "screenshot")
      ? { q: state.text, words: hit.found, ...(hit.kind === "pdf" ? { zoom: false } : {}) } : null;
    // Sharp at the screen's own pixels: up to the helper's 3000 for a page or a marked picture.
    const pixels = Math.min(3000, Math.round(Math.max(screen.width, screen.height) * (window.devicePixelRatio || 1)));
    if (hit.kind !== "pdf" && SHOWN_AS_IS.has(extensionOf(hit.path)) && !marking) image.src = fileURL(hit.path);
    else {
      const shown = viewing;
      rendered(hit.path, hit.kind === "pdf" ? "pdf" : "image", hit.kind === "pdf" ? viewing.page : null, pixels,
               marking ?? (hit.kind === "pdf" ? { zoom: false } : undefined))
        .then((path) => { if (viewing === shown) image.src = fileURL(path); })
        .catch(() => { if (hit.kind !== "pdf" && SHOWN_AS_IS.has(extensionOf(hit.path))) image.src = fileURL(hit.path); });
    }
    stage.append(image);
  } else {
    const text = textPreview(hit);
    text.classList.add("viewer-text");
    stage.append(text);
  }
  $viewer.replaceChildren(bar, stage);
  $viewer.hidden = false;
  stage.focus?.();
}

function viewerButton(icon, title, onclick) {
  const button = element("button", "viewer-button");
  button.type = "button";
  button.title = title;
  button.innerHTML = icon;
  button.onclick = onclick;
  return button;
}

function zoomBy(factor) {
  viewing.zoom = Math.min(8, Math.max(0.25, viewing.zoom * factor));
  $viewer.querySelector(".viewer-stage")?.style.setProperty("--zoom", viewing.zoom);
  const label = $viewer.querySelector(".viewer-zoom");
  if (label) label.textContent = `${Math.round(viewing.zoom * 100)}%`;
}

function turnPage(step) {
  const pages = Number(viewing.hit.info?.pages) || Infinity;
  const page = Math.min(pages, Math.max(1, viewing.page + step));
  if (page === viewing.page) return;
  viewing.page = page;
  viewing.zoom = 1;
  renderViewer();
}

/// The viewer's keys; true when the key was its.
function viewerKey(event) {
  switch (event.key) {
    case "Escape":
    case " ":
      closeViewer();
      return true;
    case "ArrowLeft":
    case "ArrowRight":
      if (viewing.hit.kind !== "pdf") return false;
      turnPage(event.key === "ArrowRight" ? 1 : -1);
      return true;
    case "+":
    case "=":
      zoomBy(1.25);
      return true;
    case "-":
    case "_":
      zoomBy(1 / 1.25);
      return true;
    case "0":
      if (!event.ctrlKey) return false;
      zoomBy(1 / viewing.zoom);
      return true;
    case "ArrowDown":
    case "ArrowUp":
    case "PageDown":
    case "PageUp": {
      // They scroll what's shown, not the results under it.
      const stage = $viewer.querySelector(".viewer-stage");
      const step = event.key.startsWith("Page") ? stage.clientHeight * 0.9 : 60;
      stage.scrollBy({ top: event.key.endsWith("Down") ? step : -step });
      return true;
    }
    default:
      return false;
  }
}

// MARK: Keys and events

// Typing motion is independent of the search timer and helper responses.
const searchMark = document.querySelector(".search-mark");
const searchOrbit = searchMark.querySelector(".search-orbit");
const searchSpark = searchMark.querySelector(".search-ai");
const searchMotionPreference = window.matchMedia("(prefers-reduced-motion: reduce)");
let searchMotionTimer = null;
let searchReturnAnimations = [];

function stopSearchMotion(immediate = false) {
  clearTimeout(searchMotionTimer);
  for (const animation of searchReturnAnimations) animation.cancel();
  searchReturnAnimations = [];
  if (!searchMark.classList.contains("is-typing")) return;
  const ringPose = getComputedStyle(searchOrbit).transform;
  const sparkPose = getComputedStyle(searchSpark).transform;
  const sparkOpacity = getComputedStyle(searchSpark).opacity;
  searchMark.classList.remove("is-typing");
  if (immediate || searchMotionPreference.matches) return;
  const options = { duration: 260, easing: "cubic-bezier(.2, .8, .2, 1)" };
  searchReturnAnimations = [
    searchOrbit.animate([{ transform: ringPose }, { transform: "none" }], options),
    searchSpark.animate([{ transform: sparkPose, opacity: sparkOpacity },
      { transform: "translate3d(0, 0, 0) scale(.2)", opacity: 0 }], options),
  ];
}

function animateSearchTyping() {
  if (!$query.value.trim() || searchMotionPreference.matches) {
    stopSearchMotion(true);
    return;
  }
  for (const animation of searchReturnAnimations) animation.cancel();
  searchReturnAnimations = [];
  searchMark.classList.add("is-typing");
  clearTimeout(searchMotionTimer);
  searchMotionTimer = setTimeout(stopSearchMotion, 900);
}

$query.addEventListener("input", animateSearchTyping);
$query.addEventListener("blur", () => stopSearchMotion());
searchMotionPreference.addEventListener("change", () => stopSearchMotion(true));
document.addEventListener("visibilitychange", () => { if (document.hidden) stopSearchMotion(true); });
window.addEventListener("pagehide", () => stopSearchMotion(true));

$query.addEventListener("input", queryChanged);

/// The same words, in code (the hint row, or Tab): the search window's Code filter, the panel's "code:" prefix.
function searchCode() {
  if (WINDOW) {
    state.filter = "code";
    renderChips();
  } else {
    $query.value = `code: ${state.text}`;
  }
  queryChanged();
  $query.focus();
}

document.addEventListener("keydown", (event) => {
  if (event.isComposing) return; // an input method is composing: its keys
  const modifier = event.ctrlKey;
  if (viewing && !event.altKey) {
    if (viewerKey(event)) { event.preventDefault(); return; }
    if (event.key.length === 1 && !modifier) closeViewer(); // typing goes back to the search
  } else if (event.key === " " && !modifier && !event.altKey && !event.shiftKey
             && (state.navigated || document.activeElement !== $query) && selectedHit() && canView(selectedHit())) {
    // Space after the arrows (or with the field not focused) views the result large; else it's a space typed.
    event.preventDefault();
    openViewer(selectedHit());
    return;
  }
  if (event.key === "Tab" && !event.shiftKey && !modifier && !event.altKey && state.codeMatches && !state.isCode) {
    event.preventDefault();
    searchCode();
    return;
  }
  switch (event.key) {
    case "Escape":
      event.preventDefault();
      stopSearchMotion(true);
      if (!WINDOW) invoke("hide_panel").catch(() => {});
      else if ($query.value) searchFor("");
      else invoke("close_window").catch(() => {});
      break;
    case "ArrowDown":
    case "ArrowUp":
      if (event.altKey || event.shiftKey || modifier) return;
      event.preventDefault();
      move((event.key === "ArrowDown" ? 1 : -1) * (state.grid ? gridColumns() : 1));
      break;
    case "ArrowLeft":
    case "ArrowRight":
      // In the grid they move between pictures; elsewhere they're the field's.
      if (!state.grid || event.altKey || event.shiftKey || modifier) return;
      event.preventDefault();
      move(event.key === "ArrowRight" ? 1 : -1);
      break;
    case "Enter":
      // Let a focused action button run its own click handler.
      if (!modifier && event.target.closest("button")) return;
      event.preventDefault();
      if (modifier) revealSelected();
      else if (state.selection === WEAKER) toggleWeaker();
      else openSelected();
      break;
    case "c":
    case "C":
      // The field's own copy when text is selected in it.
      if (!modifier || event.altKey) return;
      if (!event.shiftKey && document.activeElement === $query && $query.selectionStart !== $query.selectionEnd) return;
      if (!selectedHit()) return;
      event.preventDefault();
      copySelected(event.shiftKey);
      break;
    case "o":
    case "O":
      if (!modifier || WINDOW) return;
      event.preventDefault();
      invoke("open_search_window", { query: $query.value }).catch(failed);
      break;
  }
});

/// How many pictures a grid row holds now.
function gridColumns() {
  return Math.max(1, getComputedStyle($list).gridTemplateColumns.split(" ").filter(Boolean).length);
}

// MARK: The window's filters

function renderChips() {
  if (!$chips) return;
  $chips.hidden = !WINDOW;
  $chips.replaceChildren(...FILTERS.map(([id, label]) => {
    const chip = element("button", "chip" + (state.filter === id ? " on" : ""), label);
    chip.onclick = () => {
      state.filter = id;
      renderChips();
      queryChanged();
      $query.focus();
    };
    return chip;
  }));
}

function searchFor(query) {
  $query.value = query || "";
  $query.focus();
  $query.select();
  queryChanged();
}

if (WINDOW) {
  renderChips();
  searchFor(new URLSearchParams(location.search).get("q"));
  tauri.event.listen("search-query", ({ payload }) => searchFor(payload));
}

tauri.event.listen("panel-shown", () => {
  // The last query, selected.
  $query.focus();
  $query.select();
  ask({ op: "prewarm" }).catch(() => {});
});

tauri.event.listen("helper", ({ payload }) => {
  const { from, event } = payload;
  if (from === "search" && event.event === "encoder") {
    state.encoder = event.state;
    // Ready while words-only results show for what was typed: search it again, now with meaning.
    if (event.state === "ready" && state.phase === "words" && $query.value.trim()) queryChanged();
    else if (state.phase !== "empty") renderFooter();
  }
  if (from === "index") {
    if ((event.event === "file" || event.event === "rest") && event.total) {
      state.indexing = { done: event.done, total: event.total };
      $activity.textContent = `Indexing ${event.done}/${event.total}`;
      $activity.hidden = false;
    } else if (["idle", "waiting", "gone"].includes(event.event)) {
      state.indexing = null;
      $activity.hidden = true;
    }
    if (state.phase !== "empty") renderFooter();
  }
});

tauri.event.listen("status", ({ payload }) => {
  if (WINDOW && state.phase === "empty" && !state.error) $summary.textContent = payload;
});

tauri.path.homeDir().then((home) => { state.home = home.replace(/[\\/]$/, ""); }).catch(() => {});

// Debug runs (--debug-query): type the query in, and once the meaning results and their pictures are in, report
// what the panel shows.
function debugSearch(query) {
  if (!query) return;
  debugPending = true;
  $query.value = query;
  queryChanged();
}
if (!WINDOW) {
  invoke("debug_query").then(debugSearch);
  tauri.event.listen("debug-query", ({ payload }) => debugSearch(payload));   // after setup's first index
}

function report() {
  debugPending = false;
  setTimeout(() => {
    const big = $preview.querySelector("img, video, audio, .passage");
    const thumbs = [...$list.querySelectorAll("img.thumb")];
    invoke("debug_report", {
      report: {
        query: $query.value,
        phase: state.phase,
        groups: state.groups.map((group) => ({ title: group.title, rows: group.rows.map((hit) => nameOf(hit.path)) })),
        weaker: state.weaker.length,
        selected: state.selection && nameOf(state.selection),
        preview: big && {
          tag: big.tagName.toLowerCase(),
          loaded: big.tagName === "IMG" ? big.complete && big.naturalWidth > 0 : null,
          size: big.tagName === "IMG" ? `${big.naturalWidth}x${big.naturalHeight}` : null,
          text: big.classList.contains("passage") ? big.textContent.slice(0, 80) : null,
        },
        thumbnails: `${thumbs.filter((image) => image.complete && image.naturalWidth > 0).length} of ${thumbs.length} drawn`,
        facts: [...$preview.querySelectorAll(".facts .title, .facts .line, .reason span")].map((node) => node.textContent),
        footer: [$summary.textContent, $keys.textContent],
      },
    }).catch(() => {});
  }, 1500);
}

// The window starts with something to look at; the panel only shows its field until you type.
if (WINDOW && state.phase === "empty") render();

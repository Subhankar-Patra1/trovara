// What a text preview shows (PRD §6.1b, TRD §12.7): the part of a file that matched, with the found words marked by
// a yellow highlighter, opened at the first mark. Plain passages, code (coloured by highlight.js, with line numbers)
// and Markdown (formatted by markdown.js). The marks come from the helper, which decides them with keyword search's
// own matching; this file only draws them.
"use strict";

const TrovaraPreview = (() => {
  const escape = (text) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  // File extension → highlight.js language (the bundled build's 43). Unknown: no colour.
  const LANGUAGES = {
    py: "python", pyi: "python", pyx: "python", ipynb: "python", js: "javascript", mjs: "javascript", cjs: "javascript",
    jsx: "javascript", ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript", rs: "rust", go: "go",
    java: "java", kt: "kotlin", kts: "kotlin", swift: "swift", c: "c", h: "c", cc: "cpp", cpp: "cpp", cxx: "cpp",
    hpp: "cpp", hh: "cpp", hxx: "cpp", cu: "cpp", cuh: "cpp", ino: "cpp", cs: "csharp", m: "objectivec",
    mm: "objectivec", rb: "ruby", php: "php", pl: "perl", pm: "perl", r: "r", lua: "lua", sh: "bash", bash: "bash",
    zsh: "bash", fish: "bash", ps1: "powershell", psm1: "powershell", sql: "sql", html: "xml", htm: "xml",
    xml: "xml", xaml: "xml", svg: "xml", vue: "xml", svelte: "xml", css: "css", scss: "scss", less: "less",
    json: "json", yaml: "yaml", yml: "yaml", toml: "ini", ini: "ini", cfg: "ini", dart: "dart", scala: "scala",
    ex: "elixir", exs: "elixir", hs: "haskell", proto: "protobuf", graphql: "graphql", gql: "graphql", vb: "vbnet",
    diff: "diff", patch: "diff", mk: "makefile", gradle: "kotlin",
  };
  const NAMED = { dockerfile: "dockerfile", containerfile: "dockerfile", makefile: "makefile", gnumakefile: "makefile" };

  function languageOf(path) {
    const name = path.split(/[\\/]/).pop().toLowerCase();
    if (NAMED[name] || name.startsWith("dockerfile")) return NAMED[name] ?? "dockerfile";
    const dot = name.lastIndexOf(".");
    return dot > 0 ? LANGUAGES[name.slice(dot + 1)] ?? null : null;
  }

  function isMarkdown(path) {
    return /\.(md|markdown|mdown|mkd)$/i.test(path);
  }

  /// `code` coloured as `language`, as HTML (highlight.js escapes it), or null when the language isn't known.
  function colour(code, language) {
    const hljs = window.hljs;
    const name = language && hljs?.getLanguage(language) ? language : null;
    if (!name) return null;
    try {
      return hljs.highlight(code, { language: name, ignoreIllegals: true }).value;
    } catch {
      return null;
    }
  }

  /// Coloured HTML cut into lines, each closing the spans open at its end and opening them again on the next line,
  /// so a comment or string across lines keeps its colour on every one.
  function splitLines(html) {
    const lines = [];
    const open = [];
    let current = "";
    for (const token of html.match(/<span class="[^"]*">|<\/span>|[^<]+/g) ?? []) {
      if (token.startsWith("<span")) {
        open.push(token);
        current += token;
      } else if (token === "</span>") {
        open.pop();
        current += token;
      } else {
        const parts = token.split("\n");
        current += parts[0];
        for (const part of parts.slice(1)) {
          lines.push(current + "</span>".repeat(open.length));
          current = open.join("") + part;
        }
      }
    }
    lines.push(current);
    return lines;
  }

  /// Wraps the UTF-16 `ranges` of `container`'s text (as `textContent` reads it) in yellow marks.
  function markRanges(container, ranges) {
    if (!ranges?.length) return;
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    const nodes = [];
    for (let at = 0, node; (node = walker.nextNode()); at += node.data.length) nodes.push([node, at]);
    // From the end, so earlier offsets stay right while nodes are split.
    for (const [start, end] of [...ranges].sort((a, b) => b[0] - a[0])) {
      for (let i = nodes.length - 1; i >= 0; i--) {
        const [node, at] = nodes[i];
        const from = Math.max(start, at), to = Math.min(end, at + node.data.length);
        if (from >= to) continue;
        const range = document.createRange();
        range.setStart(node, from - at);
        range.setEnd(node, to - at);
        const mark = document.createElement("mark");
        mark.className = "found";
        try { range.surroundContents(mark); } catch { /* across elements: that piece stays unmarked */ }
      }
      // Splitting changed the nodes: read them again for the next range.
      nodes.length = 0;
      const again = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
      for (let at = 0, node; (node = again.nextNode()); at += node.data.length) nodes.push([node, at]);
    }
  }

  const fold = (text) => text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

  /// Marks `words` (the forms the helper found) wherever they start a word in `container`'s text: for formatted
  /// Markdown, whose text isn't the file's.
  function markWords(container, words) {
    const wanted = [...new Set((words ?? []).map(fold).filter((w) => w.length > 1))];
    if (!wanted.length) return;
    const text = container.textContent;
    const folded = fold(text);
    if (folded.length !== text.length) return; // a script whose letters fold into more or fewer: unmarked
    const ranges = [];
    for (const word of wanted) {
      for (let at = folded.indexOf(word); at >= 0; at = folded.indexOf(word, at + word.length)) {
        const before = at > 0 ? folded[at - 1] : " ", after = folded[at + word.length] ?? " ";
        if (/[\p{L}\p{N}_]/u.test(before) || /[\p{L}\p{N}_]/u.test(after)) continue;
        ranges.push([at, at + word.length]);
      }
    }
    ranges.sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const range of ranges) {
      if (merged.length && range[0] <= merged[merged.length - 1][1]) merged[merged.length - 1][1] = Math.max(merged[merged.length - 1][1], range[1]);
      else merged.push(range);
    }
    markRanges(container, merged);
  }

  /// Scrolls `scroller` so `target` (or the first mark) sits about a third of the way down.
  function scrollTo(scroller, target) {
    const first = target ?? scroller.querySelector("mark.found");
    if (!first) return;
    const top = first.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    scroller.scrollTop = Math.max(0, top - scroller.clientHeight / 3);
  }

  /// A plain passage: the text, its marks, opened at the first.
  function passage(target, reply) {
    target.innerHTML = escape(reply.text);
    markRanges(target, reply.marks);
    requestAnimationFrame(() => scrollTo(target));
  }

  /// Code with its line numbers from `first`, coloured, its marks, the line it opens at (`focus`) lit and scrolled to.
  function code(target, reply, path, first, focus) {
    const coloured = colour(reply.text, languageOf(path));
    const lines = coloured ? splitLines(coloured) : reply.text.split("\n").map(escape);
    // The marks per line, from the offsets over the whole text.
    const starts = [];
    for (let at = 0, i = 0; i < lines.length; i++) {
      starts.push(at);
      const next = reply.text.indexOf("\n", at);
      at = next < 0 ? reply.text.length : next + 1;
    }
    target.innerHTML = lines.map((line, index) => {
      const number = first + index;
      return `<div class="line${number === focus ? " focus" : ""}"><span class="ln">${number}</span><span class="src">${line || " "}</span></div>`;
    }).join("");
    const rows = target.querySelectorAll(".line .src");
    for (const [start, end] of reply.marks ?? []) {
      let line = starts.findIndex((s, i) => start >= s && (i + 1 >= starts.length || start < starts[i + 1]));
      if (line < 0) continue;
      // A mark can run over a line end only in prose; code's words don't: cut at the line.
      const lineEnd = line + 1 < starts.length ? starts[line + 1] - 1 : reply.text.length;
      markRanges(rows[line], [[start - starts[line], Math.min(end, lineEnd) - starts[line]]]);
    }
    requestAnimationFrame(() => scrollTo(target, target.querySelector(".line.focus") ?? null));
  }

  /// Markdown, formatted, its words marked, opened at the first.
  function markdown(target, reply) {
    target.innerHTML = TrovaraMarkdown.render(reply.text, colour);
    markWords(target, reply.marks_words);
    requestAnimationFrame(() => scrollTo(target));
  }

  return { languageOf, isMarkdown, colour, splitLines, markRanges, markWords, passage, code, markdown, escape };
})();

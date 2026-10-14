// Markdown, formatted for a preview (PRD F-PRE-06/07, TRD §12.7): headings, paragraphs, emphasis, strike-through,
// inline code, lists with tasks, quotes, fenced and indented code, tables, rules. Safe by construction: every piece of
// the file is HTML-escaped before it's formatted, so nothing in a file becomes markup or script. Links are shown, not
// followed (a span with the address as its tooltip); images become "[image: alt]" and are never fetched.
"use strict";

const TrovaraMarkdown = (() => {
  const escape = (text) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  /// Inline formatting of one escaped-to-be line of text.
  function inline(source) {
    // Code spans first: their insides are never formatted.
    const codes = [];
    let text = source.replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_, ticks, body) => {
      codes.push(`<code>${escape(body.trim())}</code>`);
      return `\u0000${codes.length - 1}\u0000`;
    });
    text = escape(text);
    // Images: never loaded. Links: shown, never followed.
    text = text.replace(/!\[([^\]]*)\]\(([^)\s]*)(?:\s+&quot;[^&]*&quot;)?\)/g, (_, alt) => `<span class="md-image">[image${alt ? `: ${alt}` : ""}]</span>`);
    text = text.replace(/\[([^\]]+)\]\(([^)\s]*)(?:\s+&quot;[^&]*&quot;)?\)/g, (_, label, url) => `<span class="md-link" title="${url}">${label}</span>`);
    text = text.replace(/&lt;((?:https?|mailto):[^\s&]+)&gt;/g, (_, url) => `<span class="md-link" title="${url}">${url}</span>`);
    text = text.replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, "<strong>$2</strong>");
    text = text.replace(/(^|[^\p{L}\p{N}*_])(\*|_)(?=\S)([\s\S]*?\S)\2(?![\p{L}\p{N}])/gu, "$1<em>$3</em>");
    text = text.replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");
    text = text.replace(/ {2,}$|\\$/gm, "<br>");
    return text.replace(/\u0000(\d+)\u0000/g, (_, i) => codes[Number(i)]);
  }

  const isRule = (line) => /^ {0,3}([-*_])( *\1){2,} *$/.test(line);
  const listItem = (line) => line.match(/^( *)([-*+]|\d{1,9}[.)]) +(.*)$/);
  const fence = (line) => line.match(/^ {0,3}(`{3,}|~{3,})\s*([\w+#.-]*)/);
  const tableRow = (line) => line.trim().replace(/^\||\|$/g, "").split("|").map((cell) => cell.trim());
  const isTableSeparator = (line) => /^ *\|? *:?-+:? *(\| *:?-+:? *)*\|? *$/.test(line) && line.includes("-");

  /// Blocks of `lines` as HTML. `highlight(code, language)` colours code blocks (or returns null).
  function blocks(lines, highlight) {
    const out = [];
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      if (!line.trim()) { i += 1; continue; }

      const opening = fence(line);
      if (opening) {
        const marker = opening[1], language = opening[2];
        const body = [];
        i += 1;
        while (i < lines.length && !new RegExp(`^ {0,3}${marker[0]}{${marker.length},}\\s*$`).test(lines[i])) {
          body.push(lines[i]);
          i += 1;
        }
        i += 1;
        const code = body.join("\n");
        const coloured = highlight ? highlight(code, language) : null;
        out.push(`<pre class="md-code"><code>${coloured ?? escape(code)}</code></pre>`);
        continue;
      }

      const heading = line.match(/^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/);
      if (heading) {
        out.push(`<h${heading[1].length}>${inline(heading[2])}</h${heading[1].length}>`);
        i += 1;
        continue;
      }

      if (isRule(line)) { out.push("<hr>"); i += 1; continue; }

      if (/^ {0,3}>/.test(line)) {
        const quoted = [];
        while (i < lines.length && lines[i].trim() && /^ {0,3}>/.test(lines[i])) {
          quoted.push(lines[i].replace(/^ {0,3}> ?/, ""));
          i += 1;
        }
        out.push(`<blockquote>${blocks(quoted, highlight)}</blockquote>`);
        continue;
      }

      if (line.includes("|") && i + 1 < lines.length && isTableSeparator(lines[i + 1])) {
        const head = tableRow(line);
        const aligns = tableRow(lines[i + 1]).map((cell) => (cell.startsWith(":") && cell.endsWith(":") ? "center" : cell.endsWith(":") ? "right" : ""));
        i += 2;
        const rows = [];
        while (i < lines.length && lines[i].includes("|") && lines[i].trim()) {
          rows.push(tableRow(lines[i]));
          i += 1;
        }
        const cell = (tag, text, index) => `<${tag}${aligns[index] ? ` style="text-align:${aligns[index]}"` : ""}>${inline(text ?? "")}</${tag}>`;
        out.push(`<table><thead><tr>${head.map((text, index) => cell("th", text, index)).join("")}</tr></thead><tbody>`
          + rows.map((row) => `<tr>${head.map((_, index) => cell("td", row[index], index)).join("")}</tr>`).join("")
          + "</tbody></table>");
        continue;
      }

      const item = listItem(line);
      if (item) {
        const indent = item[1].length;
        const ordered = /\d/.test(item[2]);
        const items = [];
        while (i < lines.length) {
          const current = listItem(lines[i]);
          if (!current || current[1].length !== indent || /\d/.test(current[2]) !== ordered) break;
          const body = [current[3]];
          i += 1;
          // The item's own continuation lines and nested lists: indented further than its marker.
          while (i < lines.length && (lines[i].trim() === "" ? (i + 1 < lines.length && /^\s+\S/.test(lines[i + 1]) && (lines[i + 1].match(/^ */)[0].length > indent))
                                                           : (lines[i].match(/^ */)[0].length > indent && !isRule(lines[i])))) {
            body.push(lines[i].slice(Math.min(lines[i].match(/^ */)[0].length, indent + 2)));
            i += 1;
          }
          let content = body.join("\n");
          let task = "";
          const box = content.match(/^\[([ xX])\]\s+/);
          if (box) {
            task = `<span class="md-task${box[1] === " " ? "" : " done"}">${box[1] === " " ? "☐" : "☑"}</span> `;
            content = content.slice(box[0].length);
          }
          const inner = content.includes("\n") ? blocks(content.split("\n"), highlight) : inline(content);
          items.push(`<li${task ? ' class="task"' : ""}>${task}${inner.replace(/^<p>([\s\S]*)<\/p>$/, "$1")}</li>`);
        }
        out.push(ordered ? `<ol>${items.join("")}</ol>` : `<ul>${items.join("")}</ul>`);
        continue;
      }

      if (/^( {4}|\t)/.test(line)) {
        const body = [];
        while (i < lines.length && (/^( {4}|\t)/.test(lines[i]) || !lines[i].trim())) {
          body.push(lines[i].replace(/^( {4}|\t)/, ""));
          i += 1;
        }
        out.push(`<pre class="md-code"><code>${escape(body.join("\n").replace(/\n+$/, ""))}</code></pre>`);
        continue;
      }

      // A paragraph, or a setext heading (a line of === or --- under it).
      const paragraph = [];
      while (i < lines.length && lines[i].trim() && !fence(lines[i]) && !/^ {0,3}(#{1,6}\s|>)/.test(lines[i])
             && !(paragraph.length && (listItem(lines[i]) || isRule(lines[i]) && !/^ {0,3}-+ *$/.test(lines[i])))) {
        if (paragraph.length && /^ {0,3}(=+|-+) *$/.test(lines[i])) {
          const level = lines[i].trim()[0] === "=" ? 1 : 2;
          out.push(`<h${level}>${inline(paragraph.join("\n"))}</h${level}>`);
          paragraph.length = 0;
          i += 1;
          break;
        }
        paragraph.push(lines[i]);
        i += 1;
      }
      if (paragraph.length) out.push(`<p>${inline(paragraph.join("\n"))}</p>`);
    }
    return out.join("");
  }

  /// `text` as HTML for a preview. `highlight(code, language)` returns coloured HTML for a code block, or null.
  function render(text, highlight) {
    return blocks(text.replace(/\r\n?/g, "\n").split("\n"), highlight);
  }

  return { render, escape };
})();

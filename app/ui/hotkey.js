// The shortcut recorder, for setup and settings: "Change…", press the keys, and the app tries them (another app may
// hold them; then the old shortcut stays). "Reset" goes back to Ctrl+Shift+Space.
"use strict";

const TrovaraHotkey = (() => {
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;
  // event.code → the key's name for the app's accelerators.
  const NAMED = {
    Space: "Space", Enter: "Enter", Tab: "Tab", Backspace: "Backspace", Delete: "Delete", Insert: "Insert",
    Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", ArrowUp: "ArrowUp", ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft", ArrowRight: "ArrowRight", Backquote: "Backquote", Minus: "Minus", Equal: "Equal",
    BracketLeft: "BracketLeft", BracketRight: "BracketRight", Backslash: "Backslash", Semicolon: "Semicolon",
    Quote: "Quote", Comma: "Comma", Period: "Period", Slash: "Slash",
  };
  const SHOWN = { Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\",
                  Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/", ArrowUp: "↑", ArrowDown: "↓",
                  ArrowLeft: "←", ArrowRight: "→" };

  /// The accelerator for a key press ("Control+Shift+Space"); null while only modifiers are down; {error} when it
  /// can't be a shortcut.
  function fromEvent(event) {
    if (MODIFIER_CODES.test(event.code)) return null;
    let key = null;
    if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
    else if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
    else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)) key = event.code;
    else if (/^Numpad[0-9]$/.test(event.code)) key = event.code;
    else key = NAMED[event.code] ?? null;
    if (!key) return { error: "That key can't be part of a shortcut." };
    const parts = [];
    if (event.ctrlKey) parts.push("Control");
    if (event.altKey) parts.push("Alt");
    if (event.shiftKey) parts.push("Shift");
    if (event.metaKey) parts.push("Super");
    const functionKey = /^F\d+$/.test(key);
    // Without Ctrl, Alt or Win it would take a key away from typing everywhere.
    if (!functionKey && !(event.ctrlKey || event.altKey || event.metaKey)) {
      return { error: "Use Ctrl, Alt or Win with it, so typing still works." };
    }
    parts.push(key);
    return { accelerator: parts.join("+") };
  }

  /// "Control+Shift+Space" → ["Ctrl", "Shift", "Space"], for key caps.
  function caps(accelerator) {
    return (accelerator || "").split("+").filter(Boolean).map((part) =>
      ({ Control: "Ctrl", CommandOrControl: "Ctrl", Super: "Win" })[part] ?? SHOWN[part] ?? part);
  }

  function showCaps(container, accelerator) {
    container.replaceChildren(...caps(accelerator).map((key) => {
      const cap = document.createElement("span");
      cap.className = "key";
      cap.textContent = key;
      return cap;
    }));
  }

  /// Wires a recorder: `keys` shows the shortcut, `change` records a new one, `reset` goes back to the default,
  /// `note` says what happened. `onChange(accelerator)` after a new shortcut is in use.
  function attach({ keys, change, reset, note, current, onChange }) {
    let accelerator = current;
    let recording = false;
    showCaps(keys, accelerator);

    async function stop(message) {
      recording = false;
      document.removeEventListener("keydown", listen, true);
      keys.classList.remove("recording");
      change.textContent = "Change…";
      await invoke("hotkey_recording", { on: false }).catch(() => {});
      showCaps(keys, accelerator);
      if (message != null) note.textContent = message;
    }

    async function use(wanted) {
      try {
        await invoke("set_hotkey", { accelerator: wanted });
        accelerator = wanted || "Control+Shift+Space";
        await stop(`Now ${caps(accelerator).join("+")} opens Trovara.`);
        note.classList.remove("problem");
        onChange?.(accelerator);
      } catch (error) {
        await stop(String(error));
        note.classList.add("problem");
      }
    }

    function listen(event) {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape" && !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey) {
        stop("");
        return;
      }
      const got = fromEvent(event);
      if (!got) {
        // Modifiers so far: show them as they're held.
        const held = [event.ctrlKey && "Control", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super"];
        showCaps(keys, held.filter(Boolean).join("+") || "…");
        return;
      }
      if (got.error) {
        note.textContent = got.error;
        note.classList.add("problem");
        return;
      }
      use(got.accelerator);
    }

    change.addEventListener("click", async () => {
      if (recording) {
        stop("");
        return;
      }
      recording = true;
      await invoke("hotkey_recording", { on: true }).catch(() => {});
      keys.classList.add("recording");
      showCaps(keys, "…");
      change.textContent = "Cancel";
      note.classList.remove("problem");
      note.textContent = "Press the new shortcut (Esc cancels).";
      document.addEventListener("keydown", listen, true);
    });
    reset?.addEventListener("click", () => use(""));
    return { set(value) { accelerator = value; showCaps(keys, value); } };
  }

  return { attach, caps, fromEvent };
})();

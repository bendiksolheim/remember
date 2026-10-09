// Rendering and input only — every rule lives in remember-core, reached
// through the wasm `Remember` wrapper (see crates/web/src/lib.rs). The UI is
// a copy of the macOS capture panel (swift/Sources/RememberMac/CaptureView.swift):
// keep the two in step.
import init, { Remember } from "./pkg/remember_web.js";

// Sample data, replayed through the core on every visit. Nothing is saved,
// so every visitor starts from the same place. Top to bottom as shown; due
// phrases at the end of a title go through the same detection as typing.
// The first list doubles as the tutorial: the panel has no visible help.
const SEED = [
  {
    list: "Tasks",
    tasks: [
      "Type “call mum tomorrow” above and press Return",
      "Press ↓ to focus a task, then Space to complete it",
      "Press E to rename the focused task",
      "Move the focused task with ⌥⇧J and ⌥⇧K",
      "⌘Z undoes anything, ⇧⌘Z redoes",
      "Water the plants today",
      "Book dentist appointment friday",
    ],
  },
  { list: "Groceries", tasks: ["Oat milk", "Coffee beans", "Bananas"] },
  { list: "Work", tasks: ["Reply to the design review tomorrow", "Prepare demo slides"] },
];

// PanelMetrics in SpotlightPanel.swift.
const ROW_HEIGHT = 40;
const MAX_VISIBLE_ROWS = 8;
// How long a checked-off row lingers before it leaves the list.
const GHOST_MS = 1000;

const $ = (id) => document.getElementById(id);
const offsetSeconds = -new Date().getTimezoneOffset() * 60;

function addCommand(engine, text) {
  const d = engine.detect_due(text);
  return { type: "Add", title: d ? d.stripped_title : text, after: null, due: d ? d.due : null, list_id: null };
}

function seeded() {
  const engine = new Remember(undefined, undefined);
  engine.set_local_offset_seconds(offsetSeconds);
  const byName = (name) => engine.snapshot().lists.find((l) => l.name === name);
  SEED.forEach(({ list, tasks }) => {
    // A fresh document already has its default "Tasks" list; reuse it.
    if (!byName(list)) {
      engine.dispatch({ type: "AddList", name: list, after: engine.snapshot().lists.at(-1).id });
    }
    engine.set_current_list(byName(list).id);
    // `after: null` inserts at the top, so add bottom-up.
    [...tasks].reverse().forEach((title) => engine.dispatch(addCommand(engine, title)));
  });
  // Reopen from the bytes so the seeding isn't on the undo stack, and so
  // the visitor lands on the first list.
  const bytes = engine.export();
  engine.free();
  const app = new Remember(bytes, undefined);
  app.set_local_offset_seconds(offsetSeconds);
  app.set_view("Active");
  return app;
}

await init();
const app = seeded();

// Commands mirror remember_core::Command; omitted Option fields are null.
function dispatch(command) {
  try {
    app.dispatch(command);
  } catch (e) {
    console.error(e);
  }
}

// --- View state (CaptureView's @State) -------------------------------------

/** `{ type: "input" }`, `{ type: "row", id }`, `{ type: "edit", id }` or `{ type: "none" }`. */
let focus = { type: "input" };
let editText = "";
let detection = null;
/** Set when the badge is clicked; cleared on the next keystroke. */
let dueDismissed = false;
/**
 * Rows just checked off, kept on screen for GHOST_MS in their old spot:
 * id → { row, anchor, timer }, where `anchor` is the id of the active row
 * the ghost followed when completed, or "" if it was first.
 */
const ghosts = new Map();

/** Active rows in order, each ghost reinserted right after its anchor. */
function displayRows(s) {
  const ghostEntries = [...ghosts.values()].map((g) => ({ anchor: g.anchor, row: g.row, completed: true }));
  const anchoredTo = (anchor) => ghostEntries.filter((g) => g.anchor === anchor);
  const anchored = [
    ...anchoredTo(""),
    ...s.rows
      .filter((r) => !ghosts.has(r.id))
      .flatMap((row) => [{ row, completed: false }, ...anchoredTo(row.id)]),
  ];
  // A ghost whose anchor is gone too has nowhere to go — put it last.
  const placed = new Set(anchored.map((e) => e.row.id));
  return [...anchored, ...ghostEntries.filter((g) => !placed.has(g.row.id))];
}

/** The input first, then every displayed row. Wraps at both ends. */
function focusChain(s) {
  return [{ type: "input" }, ...displayRows(s).map((e) => ({ type: "row", id: e.row.id }))];
}

const sameFocus = (a, b) => a.type === b.type && a.id === b.id;

function moveFocus(delta) {
  const chain = focusChain(app.snapshot());
  const i = Math.max(0, chain.findIndex((f) => sameFocus(f, focus)));
  focus = chain[(i + delta + chain.length) % chain.length];
  render();
}

/** ⌥⇧J/⌥⇧K: swap the focused row with its active neighbour; clamps at the ends. */
function moveFocusedRow(delta) {
  if (focus.type !== "row") return;
  const rows = app.snapshot().rows;
  const i = rows.findIndex((r) => r.id === focus.id);
  const n = i + delta;
  if (i < 0 || n < 0 || n >= rows.length) return;
  const after = delta > 0 ? rows[n].id : n > 0 ? rows[n - 1].id : null;
  dispatch({ type: "Move", id: focus.id, after });
  render();
}

function toggle(id) {
  focus = { type: "row", id };
  const ghost = ghosts.get(id);
  if (ghost) {
    clearTimeout(ghost.timer);
    ghosts.delete(id);
    dispatch({ type: "SetDone", id, done: false });
  } else {
    const rows = app.snapshot().rows;
    const i = rows.findIndex((r) => r.id === id);
    if (i < 0) return;
    dispatch({ type: "SetDone", id, done: true });
    ghosts.set(id, {
      row: rows[i],
      anchor: i > 0 ? rows[i - 1].id : "",
      timer: setTimeout(() => removeGhost(id), GHOST_MS),
    });
  }
  render();
}

/** If the ghost held focus, hand it to whatever came after it. */
function removeGhost(id) {
  if (focus.type === "row" && focus.id === id) {
    const chain = focusChain(app.snapshot());
    const i = chain.findIndex((f) => sameFocus(f, focus));
    focus = chain[(i + 1) % chain.length];
  }
  ghosts.delete(id);
  render();
}

function beginEdit() {
  if (focus.type !== "row" || ghosts.has(focus.id)) return;
  const row = app.snapshot().rows.find((r) => r.id === focus.id);
  if (!row) return;
  editText = row.title;
  focus = { type: "edit", id: row.id };
  render();
}

/** `SetTitle` trims and ignores an empty title itself. */
function endEdit(commit) {
  if (focus.type !== "edit") return;
  if (commit) dispatch({ type: "SetTitle", id: focus.id, title: editText });
  focus = { type: "row", id: focus.id };
  editText = "";
  render();
}

function selectList(id) {
  app.set_current_list(id);
  focus = { type: "input" };
  render();
}

// --- Rendering -------------------------------------------------------------

const ICONS = {
  circle: `<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>`,
  checked: `<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="7.65" fill="currentColor"/><path d="M4.9 8.3l2.1 2.2 4.2-4.7" fill="none" stroke="#2a2a2e" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  calendar: `<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.75" y="2.75" width="12.5" height="11.5" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M2 6.25h12" stroke="currentColor" stroke-width="1.3"/><path d="M5 1.5v2.5M11 1.5v2.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>`,
};

// pillTextColor in ListColor.swift.
const DARK_TEXT_COLORS = new Set(["Orange", "Teal", "Mint", "Yellow", "Cyan"]);

// dueChipColor in DueColor.swift.
function dueTint(state, done) {
  if (done) return "var(--secondary)";
  if (state === "Overdue") return "var(--red)";
  if (state === "Today") return "var(--orange)";
  return "var(--secondary)";
}

function chip(label, tint, tag = "span") {
  const el = document.createElement(tag);
  el.className = "chip";
  el.style.setProperty("--tint", tint);
  el.innerHTML = ICONS.calendar;
  el.append(label);
  return el;
}

// Controls the Mac panel marks `.focusable(false)`: clickable, but they
// never take keyboard focus away from the input or the focused row.
function unfocusable(el) {
  el.tabIndex = -1;
  el.addEventListener("mousedown", (e) => e.preventDefault());
  return el;
}

const input = $("input");

function render() {
  const s = app.snapshot();
  // ⌘Z can bring back a row that's still lingering as a ghost.
  const live = new Set(s.rows.map((r) => r.id));
  [...ghosts]
    .filter(([id]) => live.has(id))
    .forEach(([id, g]) => {
      clearTimeout(g.timer);
      ghosts.delete(id);
    });
  const entries = displayRows(s);
  // The focused row can vanish under us (undo, list switch): fall back to the input.
  if ((focus.type === "row" || focus.type === "edit") && !entries.some((e) => e.row.id === focus.id)) {
    focus = { type: "input" };
  }

  const current = s.lists.find((l) => l.id === s.current_list);
  document.title = current ? `${current.name} — Remember` : "Remember";

  $("badge").replaceChildren();
  if (detection && !dueDismissed) {
    const badge = unfocusable(chip(detection.label, "var(--accent)", "button"));
    badge.title = "Don’t set a due date";
    badge.onclick = () => {
      dueDismissed = true;
      render();
    };
    $("badge").append(badge);
  }

  $("pills").replaceChildren(...s.lists.map((l, i) => {
    const pill = unfocusable(document.createElement("button"));
    pill.className = l.id === s.current_list ? "pill selected" : "pill";
    pill.style.setProperty("--c", `var(--${l.color})`);
    pill.style.setProperty("--on", DARK_TEXT_COLORS.has(l.color) ? "#000" : "#fff");
    if (i < 9) {
      const key = document.createElement("span");
      key.className = "key";
      key.textContent = `⌘${i + 1}`;
      pill.append(key);
    }
    pill.append(l.name);
    pill.onclick = () => selectList(l.id);
    return pill;
  }));

  let focusEl = focus.type === "input" ? input : null;
  $("rows").replaceChildren(...entries.map(({ row, completed }) => {
    const el = document.createElement("div");
    el.className = "row";
    el.tabIndex = -1;
    if (completed) el.classList.add("completed");
    const focused = focus.type === "row" && focus.id === row.id;
    const editing = focus.type === "edit" && focus.id === row.id;
    if (focused) {
      el.classList.add("focused");
      focusEl = el;
    }

    const check = unfocusable(document.createElement("button"));
    check.className = "check";
    check.setAttribute("aria-label", completed ? "Mark as not done" : "Mark as done");
    check.innerHTML = completed ? ICONS.checked : ICONS.circle;
    check.disabled = editing;
    check.onclick = () => toggle(row.id);
    el.append(check);

    if (editing) {
      const field = document.createElement("input");
      field.type = "text";
      field.value = editText;
      field.spellcheck = false;
      field.oninput = () => (editText = field.value);
      field.onkeydown = (e) => {
        if (e.key === "Enter") endEdit(true);
        else if (e.key === "Escape") endEdit(false);
        else return;
        e.preventDefault();
      };
      el.append(field);
      focusEl = field;
    } else {
      const title = document.createElement("span");
      title.className = "title";
      title.textContent = row.title;
      el.append(title);
      if (row.due_label) el.append(chip(row.due_label, dueTint(row.due_state, completed)));
    }

    el.addEventListener("mousedown", (e) => {
      // Let clicks place the caret, and leave the button alive for its click.
      if (editing || e.target.closest(".check")) return;
      e.preventDefault();
      if (focus.type === "edit") editText = "";
      focus = { type: "row", id: row.id };
      render();
    });
    return el;
  }));

  // The rows area hugs its content up to MAX_VISIBLE_ROWS, then scrolls;
  // the panel's top edge stays put, so it grows downward like the Mac panel.
  const visible = Math.min(entries.length, MAX_VISIBLE_ROWS);
  $("rows").style.height = `${visible * ROW_HEIGHT}px`;
  $("rows-divider").hidden = entries.length === 0;

  if (focusEl && document.activeElement !== focusEl) {
    focusEl.focus({ preventScroll: true });
    if (focusEl instanceof HTMLInputElement && focusEl !== input) {
      focusEl.setSelectionRange(focusEl.value.length, focusEl.value.length);
    }
  }
  if (focusEl && focusEl !== input) focusEl.closest(".row").scrollIntoView({ block: "nearest" });
}

// --- Input -----------------------------------------------------------------

input.addEventListener("focus", () => {
  if (focus.type === "input") return;
  editText = "";
  focus = { type: "input" };
  render();
});

input.addEventListener("input", () => {
  detection = input.value ? app.detect_due(input.value) ?? null : null;
  dueDismissed = false;
  render();
});

input.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" || e.isComposing) return;
  e.preventDefault();
  if (!input.value.trim()) return;
  if (detection && !dueDismissed) {
    dispatch({ type: "Add", title: detection.stripped_title, after: null, due: detection.due, list_id: null });
  } else {
    dispatch({ type: "Add", title: input.value, after: null, due: null, list_id: null });
  }
  input.value = "";
  detection = null;
  dueDismissed = false;
  render();
});

// Clicking the "desktop" is the panel losing key status: drop any edit.
document.addEventListener("mousedown", (e) => {
  if (e.target.closest(".panel")) return;
  editText = "";
  focus = { type: "none" };
  render();
});

const NO_MODIFIERS = (e) => !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;

// Matched on `e.code`: on a Mac, ⌥J types "∆", so `e.key` is useless here.
document.addEventListener("keydown", (e) => {
  const editing = focus.type === "edit";
  const command = e.metaKey || e.ctrlKey;

  if (command && !e.altKey && e.code === "KeyZ") {
    // A text field with something in it keeps its own text undo.
    if (e.target instanceof HTMLInputElement && e.target.value) return;
    e.preventDefault();
    dispatch({ type: e.shiftKey ? "Redo" : "Undo" });
    render();
    return;
  }

  // ⌘1–⌘9. Most browsers keep these for switching tabs and never send
  // them; the pills still show the Mac shortcut.
  const digit = /^Digit([1-9])$/.exec(e.code);
  if (command && !e.altKey && !e.shiftKey && digit) {
    const list = app.snapshot().lists[Number(digit[1]) - 1];
    if (list) {
      e.preventDefault();
      selectList(list.id);
    }
    return;
  }

  if (e.altKey && !command && (e.code === "KeyJ" || e.code === "KeyK")) {
    // Swallowed even while editing, so no "∆"/"˚" lands in the field.
    e.preventDefault();
    if (editing) return;
    const delta = e.code === "KeyJ" ? 1 : -1;
    if (e.shiftKey) moveFocusedRow(delta);
    else moveFocus(delta);
    return;
  }

  if (!NO_MODIFIERS(e) || editing) return;
  if (e.code === "ArrowDown" || e.code === "ArrowUp") {
    e.preventDefault();
    moveFocus(e.code === "ArrowDown" ? 1 : -1);
  } else if (focus.type === "row" && e.code === "Space") {
    e.preventDefault();
    toggle(focus.id);
  } else if (focus.type === "row" && e.code === "KeyE") {
    e.preventDefault();
    beginEdit();
  }
});

render();

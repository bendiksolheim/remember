// Rendering and input only — every rule lives in remember-core, reached
// through the wasm `Remember` wrapper (see crates/web/src/lib.rs). The UI is
// a copy of the macOS capture panel (swift/Sources/RememberMac/CaptureView.swift):
// keep the two in step.
import init, { Remember } from "./pkg/remember_web.js";

// Sample data, replayed through the core on every visit. Nothing is saved,
// so every visitor starts from the same place. Top to bottom as shown; due
// phrases at the end of a title go through the same detection as typing. A
// task is a title, or `{ title, steps }` for one with steps.
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
      {
        title: "Group tasks into steps",
        steps: [
          "Press A on a task to add steps to it",
          "Tab tucks a task under the one above",
          "⇧Tab pulls a step back out",
        ],
      },
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

function addCommand(engine, text, parentId = null) {
  const d = engine.detect_due(text);
  return {
    type: "Add",
    title: d ? d.stripped_title : text,
    after: null,
    due: d ? d.due : null,
    list_id: null,
    parent_id: parentId,
  };
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
    [...tasks].reverse().forEach((task) => {
      const { title, steps = [] } = typeof task === "string" ? { title: task } : task;
      engine.dispatch(addCommand(engine, title));
      // The task just added is the top row; steps append in order.
      const parentId = engine.snapshot().rows[0].id;
      steps.forEach((step) => engine.dispatch(addCommand(engine, step, parentId)));
    });
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
/** Id of the task captures go into as steps (CaptureView's `addTargetID`), or null. */
let addTarget = null;
/**
 * Rows just checked off, kept on screen for GHOST_MS in their old spot:
 * id → { row, anchor, timer, block }, where `anchor` is the id of the row
 * the ghost followed, or "" if it was first, and `block` (on a task) the
 * steps completed with it.
 */
const ghosts = new Map();

/** Active rows in order, each ghost (recursively) reinserted after its anchor. */
function displayRows(s) {
  const ghostEntries = [...ghosts.values()].map((g) => ({ anchor: g.anchor, row: g.row, completed: true }));
  const anchoredTo = (anchor) =>
    ghostEntries.filter((g) => g.anchor === anchor).flatMap((g) => [g, ...anchoredTo(g.row.id)]);
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

/** ⌥⇧J/⌥⇧K: the core's `Shift` moves the row among its peers, clamped. */
function moveFocusedRow(delta) {
  if (focus.type !== "row" || !isLive(focus.id)) return;
  dispatch({ type: "Shift", id: focus.id, delta });
  render();
}

/** Tab / ⇧Tab: tuck the focused row under the task above, or pull a step back out. */
function indentFocusedRow(outdent) {
  if (focus.type !== "row" || !isLive(focus.id)) return;
  dispatch({ type: outdent ? "Outdent" : "Indent", id: focus.id });
  render();
}

/** Whether `id` is a live row, not a ghost only this view still shows. */
function isLive(id) {
  return !ghosts.has(id) && app.snapshot().rows.some((r) => r.id === id);
}

/** "a": aim the input at the focused row's task. */
function targetFocused() {
  if (focus.type !== "row" || !isLive(focus.id)) return;
  const row = app.snapshot().rows.find((r) => r.id === focus.id);
  addTarget = row.parent_id ?? row.id;
  focus = { type: "input" };
  render();
}

/** The row at `i` plus, for a task, its steps shown below it. */
function blockAt(rows, i) {
  const block = [rows[i]];
  if (rows[i].parent_id) return block;
  for (let j = i + 1; j < rows.length && rows[j].parent_id === rows[i].id; j++) block.push(rows[j]);
  return block;
}

function toggle(id) {
  focus = { type: "row", id };
  const ghost = ghosts.get(id);
  const drop = (ghostId) => {
    clearTimeout(ghosts.get(ghostId)?.timer);
    ghosts.delete(ghostId);
  };
  if (ghost) {
    // A block completed together comes back together.
    for (const ghostId of [id, ...(ghost.block ?? [])]) {
      if (!ghosts.has(ghostId)) continue;
      drop(ghostId);
      dispatch({ type: "SetDone", id: ghostId, done: false });
    }
    // Reopening a step reopens its task.
    if (ghost.row.parent_id) drop(ghost.row.parent_id);
  } else {
    const rows = app.snapshot().rows;
    const i = rows.findIndex((r) => r.id === id);
    if (i < 0) return;
    // Its open steps are completed (and ghosted) with it.
    dispatch({ type: "SetDone", id, done: true });
    const block = blockAt(rows, i);
    block.forEach((row, k) => {
      ghosts.set(row.id, {
        row,
        anchor: i + k > 0 ? rows[i + k - 1].id : "",
        timer: setTimeout(() => removeGhost(row.id), GHOST_MS),
        block: k === 0 && block.length > 1 ? block.slice(1).map((r) => r.id) : undefined,
      });
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
  addTarget = null;
  focus = { type: "input" };
  render();
}

// --- Rendering -------------------------------------------------------------

const ICONS = {
  circle: `<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>`,
  checked: `<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="7.65" fill="currentColor"/><path d="M4.9 8.3l2.1 2.2 4.2-4.7" fill="none" stroke="#2a2a2e" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  chevron: `<svg viewBox="0 0 10 10" aria-hidden="true"><path d="M3.5 1.5L7 5 3.5 8.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
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

  // The target is gone (completed, deleted, undone).
  const target = s.rows.find((r) => r.id === addTarget);
  if (!target) addTarget = null;
  $("target").replaceChildren();
  if (target) {
    const el = unfocusable(document.createElement("button"));
    el.className = "target";
    el.title = "Add tasks instead of steps";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = target.title;
    el.append(name);
    el.insertAdjacentHTML("beforeend", ICONS.chevron);
    el.onclick = () => {
      addTarget = null;
      render();
    };
    $("target").append(el);
  }
  input.placeholder = target ? "Add a step…" : "Add a task…";

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
    if (row.parent_id) el.classList.add("step");
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
      if (row.step_count > 0) {
        const progress = document.createElement("span");
        progress.className = "progress";
        progress.textContent = `${row.steps_done}/${row.step_count}`;
        el.append(progress);
      }
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
  // Backspace in the empty input drops the step target.
  if (e.key === "Backspace" && !input.value && addTarget) {
    e.preventDefault();
    addTarget = null;
    render();
    return;
  }
  if (e.key !== "Enter" || e.isComposing) return;
  e.preventDefault();
  if (!input.value.trim()) return;
  const step = { list_id: null, parent_id: addTarget };
  if (detection && !dueDismissed) {
    dispatch({ type: "Add", title: detection.stripped_title, after: null, due: detection.due, ...step });
  } else {
    dispatch({ type: "Add", title: input.value, after: null, due: null, ...step });
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
  // E.g. Escape ending a rename must not also drop the step target.
  if (e.defaultPrevented) return;
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

  // Once the visitor clicks away from the panel, plain keys belong to the
  // page again (arrows and Space scroll it).
  if (editing || focus.type === "none") return;

  // Tab / ⇧Tab indent and outdent the focused row (swallowed in the input).
  if (e.code === "Tab" && !command && !e.altKey) {
    e.preventDefault();
    indentFocusedRow(e.shiftKey);
    return;
  }
  // As on the Mac, Escape drops the step target.
  if (e.code === "Escape" && addTarget) {
    e.preventDefault();
    addTarget = null;
    render();
    return;
  }

  if (!NO_MODIFIERS(e)) return;
  if (e.code === "ArrowDown" || e.code === "ArrowUp") {
    e.preventDefault();
    moveFocus(e.code === "ArrowDown" ? 1 : -1);
  } else if (focus.type === "row" && e.code === "Space") {
    e.preventDefault();
    toggle(focus.id);
  } else if (focus.type === "row" && e.code === "KeyE") {
    e.preventDefault();
    beginEdit();
  } else if (focus.type === "row" && e.code === "KeyA") {
    e.preventDefault();
    targetFocused();
  }
});

render();

// Rendering and input only — every rule lives in remember-core, reached
// through the wasm `Remember` wrapper (see crates/web/src/lib.rs).
import init, { Remember } from "./pkg/remember_web.js";

// Sample data, replayed through the core on every visit. Nothing is saved,
// so every visitor starts from the same place. Top to bottom as shown; due
// phrases at the end of a title go through the same detection as typing.
const SEED = [
  {
    list: null,
    tasks: [
      "Try typing “call mum tomorrow” above",
      "Book dentist appointment friday",
      "Renew passport",
      { title: "Water the plants", done: true },
    ],
  },
  { list: "Groceries", tasks: ["Oat milk", "Coffee beans", "Bananas"] },
];

const $ = (id) => document.getElementById(id);
const offsetSeconds = -new Date().getTimezoneOffset() * 60;

function addCommand(engine, text) {
  const d = engine.detect_due(text);
  return { type: "Add", title: d ? d.stripped_title : text, after: null, due: d ? d.due : null, list_id: null };
}

function seeded() {
  const engine = new Remember(undefined, undefined);
  engine.set_local_offset_seconds(offsetSeconds);
  const lists = () => engine.snapshot().lists;
  for (const { list, tasks } of SEED) {
    if (list) {
      engine.dispatch({ type: "AddList", name: list, after: lists().at(-1).id });
      engine.set_current_list(lists().find((l) => l.name === list).id);
    }
    // `after: null` inserts at the top, so add bottom-up.
    for (const task of [...tasks].reverse()) {
      const { title, done } = typeof task === "string" ? { title: task } : task;
      engine.dispatch(addCommand(engine, title));
      if (done) {
        const row = engine.snapshot().rows.find((r) => r.title === title);
        engine.dispatch({ type: "SetDone", id: row.id, done: true });
      }
    }
  }
  // Reopen from the bytes so the seeding isn't on the undo stack, and so
  // the visitor lands on the first list.
  const bytes = engine.export();
  engine.free();
  const app = new Remember(bytes, undefined);
  app.set_local_offset_seconds(offsetSeconds);
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
  render();
}

let editing = null;

function render() {
  const s = app.snapshot();
  const current = s.lists.find((l) => l.id === s.current_list);

  $("list-name").textContent = current?.name ?? "";
  document.title = current ? `${current.name} — Remember` : "Remember";
  for (const b of $("views").children) b.setAttribute("aria-pressed", b.dataset.view === s.view);
  $("undo").disabled = !s.can_undo;
  $("redo").disabled = !s.can_redo;
  $("footer").textContent = `${s.active_count} remaining`;

  $("lists").replaceChildren(...s.lists.map((l) => {
    const li = document.createElement("li");
    li.className = l.id === s.current_list ? "current" : "";
    li.innerHTML = `<span class="dot"></span><span class="name"></span><span class="count"></span><button class="del" title="Delete list">✕</button>`;
    li.querySelector(".dot").style.background = `var(--${l.color})`;
    li.querySelector(".name").textContent = l.name;
    li.querySelector(".count").textContent = l.task_count || "";
    li.onclick = () => { app.set_current_list(l.id); render(); };
    li.ondblclick = () => {
      const name = prompt("Rename list", l.name)?.trim();
      if (name) dispatch({ type: "RenameList", id: l.id, name });
    };
    li.querySelector(".del").onclick = (e) => {
      e.stopPropagation();
      dispatch({ type: "DeleteList", id: l.id });
    };
    return li;
  }));

  if (s.rows.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = s.view === "Completed" ? "Nothing done yet" : "All clear";
    $("tasks").replaceChildren(li);
    return;
  }
  $("tasks").replaceChildren(...s.rows.map((t) => {
    const li = document.createElement("li");
    li.className = t.done ? "done" : "";
    li.innerHTML = `<input type="checkbox"><span class="title"></span><span class="due"></span><button class="del" title="Delete">✕</button>`;
    const box = li.querySelector("input");
    box.checked = t.done;
    box.onchange = () => dispatch({ type: "SetDone", id: t.id, done: box.checked });
    const title = li.querySelector(".title");
    if (editing === t.id) {
      const input = document.createElement("input");
      input.type = "text";
      input.value = t.title;
      const commit = () => {
        if (editing !== t.id) return;
        editing = null;
        const value = input.value.trim();
        if (value && value !== t.title) dispatch({ type: "SetTitle", id: t.id, title: value });
        else render();
      };
      input.onblur = commit;
      input.onkeydown = (e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") { editing = null; render(); }
      };
      title.append(input);
      queueMicrotask(() => input.focus());
    } else {
      title.textContent = t.title;
      title.ondblclick = () => { editing = t.id; render(); };
    }
    const due = li.querySelector(".due");
    due.textContent = t.due_label ?? "";
    due.classList.add(t.due_state);
    li.querySelector(".del").onclick = () => dispatch({ type: "Delete", id: t.id });
    return li;
  }));
}

const capture = $("capture").querySelector("input");
capture.oninput = () => {
  const d = capture.value.trim() ? app.detect_due(capture.value) : undefined;
  $("chip").hidden = !d;
  if (d) $("chip").textContent = d.label;
};
$("capture").onsubmit = (e) => {
  e.preventDefault();
  const text = capture.value.trim();
  if (!text) return;
  dispatch(addCommand(app, text));
  capture.value = "";
  $("chip").hidden = true;
};

$("new-list").onsubmit = (e) => {
  e.preventDefault();
  const input = e.target.querySelector("input");
  const name = input.value.trim();
  if (!name) return;
  dispatch({ type: "AddList", name, after: null });
  input.value = "";
};

for (const b of $("views").children) {
  b.onclick = () => { app.set_view(b.dataset.view); render(); };
}
$("undo").onclick = () => dispatch({ type: "Undo" });
$("redo").onclick = () => dispatch({ type: "Redo" });
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "z") return;
  if (e.target instanceof HTMLInputElement && e.target.value) return;
  e.preventDefault();
  dispatch({ type: e.shiftKey ? "Redo" : "Undo" });
});

render();

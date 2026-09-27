// The overlay: the latest reply in your language, on top of the other windows. It has no
// title bar, so it is dragged by its body and resized from its corner.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);

// Where it comes from: "Claude Code · terminal · shop (main)", then the session's title.
function from(o) {
  const project = o.project && o.branch ? `${o.project} (${o.branch})` : o.project;
  return [o.program, o.runs_in, project].filter(Boolean).join(" · ") || "Lody";
}

// The whole reply in your language when there is one, else what was read aloud.
function show(reply) {
  if (!reply) return;
  const o = reply.origin || {};
  $("from").textContent = from(o);
  $("title").textContent = o.title || "";
  document.querySelector("header").title = [from(o), o.title, o.session && `Session ${o.session}`].filter(Boolean).join("\n");
  $("text").textContent = reply.display || reply.spoken || reply.original;
  $("text").classList.remove("empty");
  $("text").scrollTop = 0;
}

invoke("latest_reply").then(show);
listen("reply", ({ payload }) => show(payload));

// Drag from anywhere but the buttons, the resize corner and the text's scroll bar.
document.addEventListener("mousedown", (e) => {
  if (e.button !== 0 || e.target.closest("button, #grip")) return;
  if (e.target.clientWidth && e.offsetX > e.target.clientWidth) return;
  win.startDragging();
});

$("grip").addEventListener("mousedown", (e) => {
  e.preventDefault();
  win.startResizeDragging("SouthEast");
});

$("close").addEventListener("click", () => invoke("set_overlay", { on: false }));

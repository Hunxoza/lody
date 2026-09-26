// Lody's settings window. Lody's own settings save as you change them; Handy's are only shown
// here and changed in Handy.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let state = null; // Lody: settings, locales, status
let handy = null; // Handy: program, config, models

function toast(text, error = false) {
  const el = $("toast");
  el.textContent = text;
  el.className = error ? "error" : "";
  el.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (el.hidden = true), error ? 7000 : 2500);
}

function option(value, label, selected) {
  const o = document.createElement("option");
  o.value = value;
  o.textContent = label;
  o.selected = selected;
  return o;
}

// A voice is `engine:name`; a bare name is an Edge voice (as older settings wrote it).
const ENGINES = ["edge", "google", "system", "command", "openai"];
const fullId = (v) => (ENGINES.includes(v.split(":")[0]) && v.includes(":") ? v : `edge:${v}`);
const voiceName = (v) => v.replace(/^[a-z]+:/, "").replace(/^[a-z]{2}-[A-Z]{2}-/, "").replace(/Neural$/, "");
const rateNumber = (r) => parseInt(String(r).replace("%", ""), 10) || 0;
const rateText = (n) => (n >= 0 ? `+${n}%` : `${n}%`);
const mb = (n) => (n >= 1000 ? `${(n / 1000).toFixed(1)} GB` : `${n} MB`);

// --- Tabs ---

document.querySelectorAll("nav button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll("nav button").forEach((x) => x.classList.toggle("active", x === b));
    document.querySelectorAll(".tab").forEach((t) => (t.hidden = t.id !== `tab-${b.dataset.tab}`));
    if (b.dataset.tab === "handy") loadHandy();
    if (b.dataset.tab === "corrections") loadCorrections();
  }),
);

// --- Lody's settings ---

function currentLocale() {
  return state.locales.find((l) => l.code === state.settings.locale) || state.locales[0];
}

function render() {
  const s = state.settings;
  const locale = currentLocale();

  const reading = state.programs.filter((p) => isOn(s, p.id)).map((p) => p.name);
  $("status").textContent = state.paused
    ? "Paused: replies are not read."
    : reading.length
      ? `Reading ${reading.join(" and ")} in ${locale.name}.`
      : "Not listening to any program.";
  $("pause").textContent = state.paused ? "Resume reading" : "Pause reading";

  const localeSelect = $("locale");
  localeSelect.replaceChildren(
    ...state.locales.map((l) => option(l.code, `${l.native_name} (${l.name})`, l.code === s.locale)),
  );
  $("translate").checked = s.translate;
  $("timeout").value = s.timeout;
  renderTranslators(s);
  renderSources(s);

  $("speech-enabled").checked = s.speech.enabled;
  $("scope").value = s.speech.scope;
  $("announce").value = s.speech.announce_project;
  $("progress").value = s.speech.progress;
  $("progress-every").value = s.speech.progress_every;
  $("max-chars").value = s.speech.max_chars;
  $("wait-handy").checked = s.speech.wait_for_handy;
  $("mute-others").checked = s.speech.mute_others;
  renderVoices(s, locale);
  const rate = rateNumber(s.speech.rate || locale.rate);
  $("rate").value = rate;
  $("rate-label").textContent = rate === 0 ? "normal" : rateText(rate);

  $("in-menu").checked = state.in_menu;
  $("in-menu").disabled = state.packaged; // the installed package keeps Lody in the menu
  $("at-login").checked = state.at_login;
  $("config-path").textContent = state.config_path;
  $("player").textContent = state.player
    ? `Audio plays through ${state.player}.`
    : "No audio player found: install mpv or ffmpeg to hear replies.";
  $("display").checked = s.display.enabled;
  $("corr-enabled").checked = s.corrections.enabled;
  $("corr-audio").checked = s.corrections.keep_audio;
  $("corr-audio").disabled = !s.corrections.enabled;
}

// --- Programs Lody reads from: one switch each (sources::PROGRAMS) ---

// A program not in the settings is read.
const isOn = (s, id) => s.sources[id] !== false;

function renderSources(s) {
  $("sources").replaceChildren(...state.programs.map((p) => {
    const note = p.found ? p.how : `${p.name} was not found on this computer. Start it once, then come back.`;
    const input = el("input", { type: "checkbox", className: "switch", checked: isOn(s, p.id) });
    input.dataset.source = p.id;
    input.addEventListener("change", save);
    return el("label", { className: "row" }, el("span", {}, p.name, el("small", { textContent: note })), input);
  }));
}

// --- Translators: one list, grouped by who makes them, each with its note ---

function renderTranslators(s) {
  const groups = new Map();
  for (const t of state.translators) {
    if (!groups.has(t.engine)) groups.set(t.engine, el("optgroup", { label: t.engine }));
    groups.get(t.engine).append(option(t.id, `${t.best ? "⭐ " : ""}${t.name}`, false));
  }
  const select = $("translator");
  select.replaceChildren(...groups.values());
  if (!state.translators.some((t) => t.id === s.translator)) select.append(option(s.translator, s.translator, false)); // set by hand
  select.value = s.translator;
  renderTranslatorNote();
}

function renderTranslatorNote() {
  const entry = state.translators.find((t) => t.id === $("translator").value);
  if (!entry) {
    $("translator-note").replaceChildren(el("small", { textContent: "A translator set in the settings file." }));
    return;
  }
  const tags = el("div", { className: "tags" });
  const tag = (t, good) => tags.append(el("span", { className: `tag${good ? " good" : ""}`, textContent: t }));
  if (entry.best) tag("Suggested", true);
  tag(`Speed ${entry.speed}/5`, entry.speed >= 4);
  tag(`Quality ${entry.quality}/5`, entry.quality >= 4);
  if (entry.needs) tag(`Needs ${entry.needs}`, false);
  $("translator-note").replaceChildren(el("small", { textContent: entry.note }), tags);
}

// --- Voices: one list, grouped by who makes them, each with its note ---

// The catalog entry a saved voice falls under (any OpenAI-compatible voice is one entry).
function voiceEntry(locale, id) {
  return locale.voices.find((v) => v.id === id) || (id.startsWith("openai:") && locale.voices.find((v) => v.setup === "openai"));
}

function renderVoices(s, locale) {
  const chosen = fullId(s.speech.voice || locale.voice);
  const groups = new Map();
  for (const v of locale.voices) {
    if (!groups.has(v.engine)) groups.set(v.engine, el("optgroup", { label: v.engine }));
    groups.get(v.engine).append(option(v.id, `${v.best ? "⭐ " : ""}${v.name}`, false));
  }
  const select = $("voice");
  select.replaceChildren(...groups.values());
  const entry = voiceEntry(locale, chosen);
  if (!entry) select.append(option(chosen, voiceName(chosen), false)); // a voice set by hand
  select.value = entry ? entry.id : chosen;
  $("voice-command").value = s.voices.command;
  $("voice-openai-url").value = s.voices.openai.url;
  $("voice-openai-model").value = s.voices.openai.model;
  $("voice-openai-key").value = s.voices.openai.key;
  $("voice-openai-voice").value = chosen.startsWith("openai:") ? chosen.slice(7) : "";
  renderVoiceNote(locale);
}

function renderVoiceNote(locale) {
  const entry = voiceEntry(locale, $("voice").value);
  const setup = entry ? entry.setup : null;
  $("voice-command-row").hidden = setup !== "command";
  ["url", "model", "key"].forEach((f) => ($(`voice-openai-${f}-row`).hidden = setup !== "openai"));
  if (!entry) {
    $("voice-note").replaceChildren(el("small", { textContent: "A voice set in the settings file." }));
    return;
  }
  const tags = el("div", { className: "tags" });
  const tag = (t, good) => tags.append(el("span", { className: `tag${good ? " good" : ""}`, textContent: t }));
  if (entry.best) tag("Suggested", true);
  if (entry.speed) tag(`Speed ${entry.speed}/5`, entry.speed >= 4);
  tag(entry.natural ? `Natural ${entry.natural}/5` : "Natural: not rated", entry.natural >= 4);
  tag(entry.online ? "Needs internet" : "Works offline", !entry.online);
  $("voice-note").replaceChildren(el("small", { textContent: entry.note }), tags);
}

// --- Read tab ---

// Text with ``` code fences: prose keeps its line breaks, code goes in a box.
function richText(text, cls) {
  const box = document.createElement("div");
  box.className = `text ${cls}`;
  text.split(/^```[^\n]*\n?/m).forEach((part, i) => {
    if (i % 2) box.append(el("pre", { textContent: part.replace(/\n$/, "") }));
    else if (part.trim()) box.append(document.createTextNode(part.replace(/^\n+|\n+$/g, "")));
  });
  return box;
}

function replyCard(r) {
  const when = new Date(r.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const shown = r.display || r.spoken || r.original;
  const text = richText(shown, "");
  const original = richText(r.original, "original");
  original.hidden = true;
  const tools = el("div", { className: "tools" });
  if (r.translated) {
    const toggle = button("English", () => {
      original.hidden = !original.hidden;
      toggle.textContent = original.hidden ? "English" : "Hide English";
    }, "secondary");
    tools.append(toggle);
  }
  tools.append(
    button("▶ Read again", () => invoke("read_again", { id: r.id }).catch((e) => toast(String(e), true)), "secondary"),
    button("Copy", () => navigator.clipboard.writeText(shown).then(() => toast("Copied")), "secondary"),
  );
  const meta = el("span", { className: "meta" }, el("b", { textContent: r.project || "reply" }), ` · ${when}`);
  return el("article", { className: "reply" }, el("header", {}, meta, tools), text, original);
}

function showReply(r) {
  $("read-empty").hidden = true;
  $("feed").prepend(replyCard(r));
  while ($("feed").children.length > 50) $("feed").lastChild.remove();
}

// Read the form back into settings, the way the core expects them.
function formSettings() {
  const s = structuredClone(state.settings);
  const locale = state.locales.find((l) => l.code === $("locale").value) || currentLocale();
  s.locale = locale.code;
  s.translate = $("translate").checked;
  s.display.enabled = $("display").checked;
  s.corrections.enabled = $("corr-enabled").checked;
  s.corrections.keep_audio = $("corr-audio").checked;
  s.timeout = Math.max(2, parseInt($("timeout").value, 10) || 8);
  s.translator = $("translator").value;
  document.querySelectorAll("[data-source]").forEach((i) => (s.sources[i.dataset.source] = i.checked));
  s.speech.enabled = $("speech-enabled").checked;
  s.speech.scope = $("scope").value;
  s.speech.announce_project = $("announce").value;
  s.speech.progress = $("progress").value;
  s.speech.progress_every = Math.min(60, Math.max(1, parseInt($("progress-every").value, 10) || 4));
  s.speech.max_chars = Math.max(100, parseInt($("max-chars").value, 10) || 1500);
  s.speech.wait_for_handy = $("wait-handy").checked;
  s.speech.mute_others = $("mute-others").checked;
  let voice = $("voice").value;
  if (voice.startsWith("openai:")) voice = `openai:${$("voice-openai-voice").value.trim() || "alloy"}`;
  s.speech.voice = voice === locale.voice ? "" : voice;
  s.voices.command = $("voice-command").value.trim();
  s.voices.openai.url = $("voice-openai-url").value.trim();
  s.voices.openai.model = $("voice-openai-model").value.trim();
  s.voices.openai.key = $("voice-openai-key").value.trim();
  const rate = parseInt($("rate").value, 10);
  s.speech.rate = rateText(rate) === locale.rate || (rate === 0 && !locale.rate) ? "" : rateText(rate);
  return s;
}

async function save() {
  const settings = formSettings();
  try {
    await invoke("save_settings", { settings });
    const localeChanged = settings.locale !== state.settings.locale;
    state.settings = settings;
    if (localeChanged) handy = null;
    render();
    toast("Saved");
  } catch (e) {
    toast(String(e), true);
  }
}

["locale", "translate", "display", "corr-enabled", "corr-audio", "timeout", "translator", "speech-enabled", "scope", "voice", "announce", "progress", "progress-every", "max-chars", "wait-handy", "mute-others", "voice-command", "voice-openai-url", "voice-openai-model", "voice-openai-voice", "voice-openai-key"].forEach(
  (id) => $(id).addEventListener("change", save),
);
$("voice").addEventListener("input", () => renderVoiceNote(currentLocale()));
$("translator").addEventListener("input", renderTranslatorNote);
$("rate").addEventListener("input", () => {
  const n = parseInt($("rate").value, 10);
  $("rate-label").textContent = n === 0 ? "normal" : rateText(n);
});
$("rate").addEventListener("change", save);

$("test").addEventListener("click", async () => {
  $("test").disabled = true;
  $("test-text").textContent = "Translating…";
  try {
    $("test-text").textContent = await invoke("test_voice", { settings: formSettings() });
  } catch (e) {
    $("test-text").textContent = "";
    toast(String(e), true);
  }
  $("test").disabled = false;
});

$("pause").addEventListener("click", async () => {
  state.paused = !state.paused;
  await invoke("set_paused", { paused: state.paused });
  render();
});
$("stop").addEventListener("click", () => invoke("stop_speaking"));
$("quit").addEventListener("click", () => invoke("quit"));

$("in-menu").addEventListener("change", async (e) => {
  try {
    await invoke("set_in_menu", { on: e.target.checked });
    toast(e.target.checked ? "Lody is in the app menu" : "Removed from the app menu");
  } catch (err) {
    e.target.checked = !e.target.checked;
    toast(String(err), true);
  }
});
$("at-login").addEventListener("change", async (e) => {
  try {
    await invoke("set_at_login", { on: e.target.checked });
    toast(e.target.checked ? "Lody will start when you log in" : "Lody won't start at login");
  } catch (err) {
    e.target.checked = !e.target.checked;
    toast(String(err), true);
  }
});

// --- Handy (read only: changes are made in Handy's own window) ---

const PASTE = {
  direct: "Types it",
  ctrl_v: "Pastes with Ctrl+V",
  ctrl_shift_v: "Pastes with Ctrl+Shift+V",
  shift_insert: "Pastes with Shift+Insert",
  none: "Only copies it: you paste",
  external_script: "Runs your own script",
};

async function loadHandy() {
  try {
    handy = await invoke("handy_state");
  } catch (e) {
    toast(String(e), true);
    return;
  }
  renderHandy();
}

function el(tag, props = {}, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

function button(label, onClick, cls = "") {
  const b = el("button", { textContent: label, className: cls });
  b.addEventListener("click", () => onClick(b));
  return b;
}

function renderHandy() {
  const status = el("span");
  const actions = el("div", { className: "actions" });
  if (!handy.program) {
    status.append(
      el("b", { textContent: "Handy is not installed" }),
      el("small", { textContent: `Lody installs Handy ${handy.release}; you'll be asked for your password.` }),
    );
    if (handy.can_install) actions.append(button("Install Handy", installHandy));
    actions.append(button("Download page", () => invoke("open_link", { url: handy.releases_url }), "secondary"));
  } else {
    status.append(
      el("b", { textContent: handy.running ? "Handy is running" : "Handy is installed, not running" }),
      el("small", { textContent: handy.who_translates }),
    );
    actions.append(button("Open Handy", openHandy));
  }
  $("handy-program").replaceChildren(el("div", { className: "row" }, status, actions));

  $("handy-problems").hidden = handy.problems.length === 0;
  $("handy-problems").replaceChildren(
    el("b", { textContent: "Change this in Handy" }),
    el("ul", {}, ...handy.problems.map((p) => el("li", { textContent: p }))),
  );

  const c = handy.config;
  $("handy-setup").hidden = !handy.program;
  const line = (label, value, ok) =>
    el("div", { className: "row" }, el("span", { textContent: label }), el("b", { className: ok === false ? "bad" : "", textContent: value }));
  $("handy-now").replaceChildren(
    ...(c
      ? [
          line("Model", handy.model_name || c.model || "none", !!handy.model_name),
          line("Listens for", c.language === "auto" ? "Detects automatically" : c.language === handy.language ? handy.language_name : c.language, c.language === "auto" || c.language === handy.language),
          line("Translate to English", c.translate_to_english ? "On" : "Off"),
          line("Puts the text in", PASTE[c.paste_method] || c.paste_method),
        ]
      : [el("div", { className: "row" }, el("span", { textContent: "Open Handy once so it creates its settings." }))]),
  );

  $("h-language-name").textContent = handy.language_name;
  $("models").replaceChildren(...handy.models.map(modelCard));
}

function modelCard(m) {
  const tags = el("div", { className: "tags" });
  const tag = (t, good) => tags.append(el("span", { className: `tag${good ? " good" : ""}`, textContent: t }));
  if (m.chosen) tag("In use", true);
  if (m.suggested) tag("Suggested", true);
  tag(m.translate ? "Speech → English" : `Types ${handy.language_name}`, m.translate);
  tag(`Accuracy ${m.accuracy}`);
  tag(`Speed ${m.speed}`);
  tag(mb(m.size_mb));
  if (m.downloaded) tag("Downloaded");
  return el(
    "div",
    { className: `model${m.chosen ? " chosen" : ""}` },
    el("div", {}, el("div", { className: "name", textContent: m.name }), el("small", { textContent: m.description }), tags),
  );
}

async function installHandy(b) {
  b.disabled = true;
  b.textContent = "Installing…";
  try {
    await invoke("handy_install");
    toast("Handy is installed");
  } catch (e) {
    toast(String(e), true);
  }
  loadHandy();
}

async function openHandy() {
  try {
    await invoke("handy_open");
    setTimeout(loadHandy, 1500);
  } catch (e) {
    toast(String(e), true);
  }
}

// Coming back from Handy's window: show what changed there.
window.addEventListener("focus", () => {
  if (!$("tab-handy").hidden) loadHandy();
});

// --- Corrections ---

const KIND = { same: "Sent as heard", fix: "Fixed", rewrite: "Reworded" };

async function loadCorrections() {
  let c;
  try {
    c = await invoke("corrections");
  } catch (e) {
    toast(String(e), true);
    return;
  }
  const checked = c.same + c.fixed;
  const stat = (n, label) => el("div", { className: "stat" }, el("b", { textContent: n }), el("span", { textContent: label }));
  $("corr-stats").replaceChildren(
    stat(c.same, "sent as heard"),
    stat(c.fixed, "fixed by you"),
    stat(c.rewritten, "reworded"),
    stat(checked ? `${Math.round((100 * c.same) / checked)}%` : "–", "heard right"),
  );

  const words = $("corr-words");
  if (c.suggestions.length) {
    const chips = el("div", { className: "chips" }, ...c.suggestions.map(([w, n]) => el("span", { className: "chip" }, w, el("small", { textContent: `×${n}` }))));
    const copy = button("Copy all", () =>
      navigator.clipboard.writeText(c.suggestions.map(([w]) => w).join("\n")).then(() => toast("Copied: paste them in Handy's Custom Words")),
    "secondary");
    words.replaceChildren(chips, el("div", { className: "buttons" }, copy));
  } else {
    words.replaceChildren(el("p", { className: "muted", textContent: "No suggestions yet: English words you correct in Handy's text show up here." }));
  }

  $("corr-folder").textContent = "";
  $("corr-folder").append(`${c.pairs.length ? c.pairs.length + " kept · " : ""}`, Object.assign(el("a", { href: "#", textContent: "open folder" }), {
    onclick: (e) => {
      e.preventDefault();
      invoke("open_corrections_folder").catch((err) => toast(String(err), true));
    },
  }));
  const list = $("corr-list");
  list.replaceChildren(...(c.pairs.length ? c.pairs.map(pairCard) : [el("p", { className: "muted", textContent: "Nothing yet. Speak with Handy into Claude Code, fix what it got wrong, and press Enter." })]));
}

function pairCard(p) {
  const when = new Date(p.at * 1000).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  const tag = el("span", { className: `tag${p.kind === "fix" ? " good" : ""}`, textContent: KIND[p.kind] || p.kind });
  const meta = el("span", { className: "meta" }, el("b", { textContent: p.project || "prompt" }), ` · ${when} `, tag, p.post_processed ? " · via Super+E" : "", p.audio ? " · 🎙" : "");
  const remove = button("Delete", async () => {
    await invoke("correction_remove", { id: p.id }).catch((e) => toast(String(e), true));
    loadCorrections();
  }, "secondary");
  const spans = el("div", { className: "spans" }, ...p.spans.slice(0, 12).map((s) => el("div", {}, el("del", { textContent: s.from || "∅" }), " → ", el("ins", { textContent: s.to || "∅" }))));
  const full = el("details", {}, el("summary", { textContent: "Both texts" }), el("p", { textContent: `Heard: ${p.heard}` }), el("p", { textContent: `Sent: ${p.sent}` }));
  return el("article", { className: "reply" }, el("header", {}, meta, el("div", { className: "tools" }, remove)), p.spans.length ? spans : "", full);
}

listen("correction", () => {
  if (!$("tab-corrections").hidden) loadCorrections();
});

// --- Events from the app ---

listen("reply", ({ payload }) => showReply(payload));
listen("paused", ({ payload }) => {
  state.paused = payload;
  render();
});

(async () => {
  state = await invoke("state");
  render();
  // Oldest first, so the newest ends up on top.
  [...state.history].reverse().forEach(showReply);
})();

// Keeps /admin/assistant current: polls /admin/assistant/status.json and shows what the server
// says. The script decides nothing: the server sends the poll interval, the step and time
// texts, the numbers and the signature of the page's state; this sets textContent, value and
// max only (never markup). A changed signature reloads the page, at most once per 5 s and
// never over typed input. Without JavaScript the page is the same, refreshed by reloading it.
(() => {
  const root = document.getElementById("assistant-live");
  if (!root) return;
  const seen = root.dataset.signature || "";
  const note = document.getElementById("live-note");
  const stop = document.getElementById("live-stop");
  const RELOAD_KEY = "assistant-reloaded-at";
  const RELOAD_GAP_MS = 5000;
  let delay = Number(root.dataset.pollMs) || 15000;
  let timer = null;
  let stopped = false;
  let inFlight = false;

  function say(text) {
    if (note) note.textContent = text;
  }

  function halt(text) {
    stopped = true;
    if (timer !== null) window.clearTimeout(timer);
    timer = null;
    if (stop) stop.hidden = true;
    say(text);
  }

  // Typed input is never lost: a focused field, or a field changed from what the server sent.
  function editing() {
    const active = document.activeElement;
    if (active && active.matches("input, textarea, select") && active.type !== "hidden") {
      return true;
    }
    for (const field of document.querySelectorAll("input, textarea, select")) {
      if (field.type === "hidden" || field.type === "button" || field.type === "submit") continue;
      if (field.type === "checkbox" || field.type === "radio") {
        if (field.checked !== field.defaultChecked) return true;
      } else if (field.tagName === "SELECT") {
        if (Array.from(field.options).some((o) => o.selected !== o.defaultSelected)) return true;
      } else if (field.value !== field.defaultValue) {
        return true;
      }
    }
    return false;
  }

  function reloadedRecently() {
    try {
      const at = Number(window.sessionStorage.getItem(RELOAD_KEY));
      return at > 0 && Date.now() - at < RELOAD_GAP_MS;
    } catch {
      return false;
    }
  }

  function reload() {
    try {
      window.sessionStorage.setItem(RELOAD_KEY, String(Date.now()));
    } catch {
      /* Without storage the page may reload on every change. */
    }
    window.location.replace("/admin/assistant");
  }

  function show(progress) {
    const text = document.getElementById("progress-text");
    const time = document.getElementById("progress-time");
    const bar = document.getElementById("progress-bar");
    if (!progress || !text) return;
    // The live element changes only when the step does.
    if (text.textContent !== progress.text) text.textContent = progress.text;
    if (time) time.textContent = progress.time;
    if (bar) {
      bar.max = progress.steps;
      bar.value = progress.done;
    }
  }

  function showDownload(download) {
    const bar = download && document.getElementById("download-" + download.id);
    if (bar && download.total > 0) {
      bar.max = download.total;
      bar.value = download.received;
    }
  }

  function schedule() {
    if (stopped || document.hidden) return;
    if (timer !== null) window.clearTimeout(timer);
    timer = window.setTimeout(poll, delay);
  }

  async function poll() {
    timer = null;
    if (stopped || document.hidden || inFlight) return;
    inFlight = true;
    try {
      await request();
    } finally {
      inFlight = false;
    }
  }

  async function request() {
    let state;
    try {
      const response = await fetch("/admin/assistant/status.json", {
        credentials: "same-origin",
        cache: "no-store",
      });
      if (stopped) return;
      if (response.status === 401 || response.status === 403 || response.redirected) {
        halt("Signed out: reload to sign in");
        return;
      }
      if (!response.ok) {
        schedule();
        return;
      }
      state = await response.json();
      if (stopped) return;
    } catch {
      // The next poll tries again.
      schedule();
      return;
    }
    if (!state.available) {
      halt("");
      return;
    }
    if (typeof state.poll_ms === "number" && state.poll_ms > 0) delay = state.poll_ms;
    if (state.signature !== seen) {
      if (editing()) {
        say("The state changed: reload");
      } else if (!reloadedRecently()) {
        reload();
        return;
      }
    } else {
      say("");
    }
    show(state.progress);
    showDownload(state.download);
    schedule();
  }

  if (stop) {
    stop.hidden = false;
    stop.addEventListener("click", () => halt("Updating stopped: reload to resume"));
  }
  document.addEventListener("visibilitychange", () => {
    if (stopped) return;
    if (document.hidden) {
      if (timer !== null) window.clearTimeout(timer);
      timer = null;
    } else {
      if (timer !== null) window.clearTimeout(timer);
      poll();
    }
  });
  schedule();
})();

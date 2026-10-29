// Polls /admin/assistant/downloads.json while a review or a download runs: the progress bar
// moves, and the page reloads when a state (not a byte count) changes. Without JavaScript the
// page is the same, refreshed by reloading it.
(() => {
  const panel = document.getElementById("downloads");
  if (!panel || panel.dataset.poll !== "1") return;
  const seen = panel.dataset.signature || "";
  async function poll() {
    try {
      const response = await fetch("/admin/assistant/downloads.json", { credentials: "same-origin" });
      if (!response.ok) return;
      const state = await response.json();
      if (!state.available) return;
      if (state.signature !== seen) {
        window.location.reload();
        return;
      }
      const bar = state.download && document.getElementById("download-" + state.download.id);
      if (bar && state.download.total > 0) {
        bar.max = state.download.total;
        bar.value = state.download.received;
      }
    } catch {
      /* The next poll tries again. */
    }
  }
  window.setInterval(poll, 2000);
})();

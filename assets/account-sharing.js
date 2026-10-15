// Mimik-specific adapters over native URL, clipboard and Intl facilities.
(() => {
  const fragment = window.location.hash;
  let invitation = "";
  if (fragment && (location.pathname === "/auth/enroll" || location.pathname === "/auth/reset")) {
    // Fragments never enter HTTP requests; remove the address/history copy promptly.
    history.replaceState(null, "", location.pathname + location.search);
    if ((location.pathname === "/auth/enroll" || location.pathname === "/auth/reset") && /^#token=[0-9a-f]{64}$/.test(fragment)) {
      invitation = fragment.slice(7);
    }
  }
  function ready() {
    const token = document.querySelector('input[name="token"]');
    if (token && invitation) token.value = invitation;
    invitation = "";
    for (const date of document.querySelectorAll("time[data-local-time]")) {
      const value = new Date(date.dateTime);
      if (date.dateTime && Number.isFinite(value.getTime())) {
        try {
          date.textContent = new Intl.DateTimeFormat(undefined, {
            year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", timeZoneName: "short"
          }).format(value);
        } catch { /* Keep the readable UTC fallback. */ }
      }
    }
    for (const button of document.querySelectorAll("button[data-copy-target]")) {
      button.addEventListener("click", async () => {
        const field = document.getElementById(button.dataset.copyTarget);
        const status = document.getElementById("copy-status");
        if (!field || !status) return;
        let copied = false;
        if (window.isSecureContext && navigator.clipboard?.writeText) {
          try { await navigator.clipboard.writeText(field.value); copied = true; } catch { /* Selected-field fallback below. */ }
        }
        if (!copied) {
          field.focus(); field.select(); field.setSelectionRange(0, field.value.length);
          try { copied = document.execCommand("copy"); } catch { copied = false; }
        }
        status.textContent = copied ? "Link copied." : "Link selected. Use your browser’s Copy command.";
      });
    }
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready, {once: true});
  else ready();
})();

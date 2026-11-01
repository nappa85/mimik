"use strict";

// Fixed selectors and text nodes only: strict CSP/Trusted Types permit no HTML sink.
const progress = document.getElementById("upload-progress");
if (progress && ["queued", "running"].includes(progress.dataset.state)) {
  const poll = () => {
    // Network document parsing has no string-to-HTML injection sink. Copy fixed
    // text only from the inert fragment document; never insert received markup.
    const request = new XMLHttpRequest();
    request.open("GET", progress.dataset.progressUrl);
    request.responseType = "document";
    request.timeout = 10000;
    request.onload = () => {
      const section = request.status === 200
        ? request.responseXML?.getElementById("upload-progress") : null;
      if (section && !["queued", "running"].includes(section.dataset.state)) {
        window.location.reload();
        return;
      }
      const paragraph = progress.querySelector("p");
      const updated = section?.querySelector("p");
      if (paragraph && updated) paragraph.textContent = updated.textContent;
      window.setTimeout(poll, 2000);
    };
    request.onerror = request.ontimeout = () => window.setTimeout(poll, 2000);
    request.send();
  };
  window.setTimeout(poll, 2000);
}
for (const controls of document.querySelectorAll(".upload-zoom")) {
  controls.hidden = false;
  const image = controls.nextElementSibling.querySelector("img");
  for (const button of controls.querySelectorAll("button")) {
    button.addEventListener("click", () => {
      const zoom = button.dataset.uploadZoom;
      image.style.maxWidth = zoom === "fit" ? "100%" : "none";
      image.style.width = zoom === "fit" ? "100%" : `${image.naturalWidth * Number(zoom)}px`;
      for (const other of controls.querySelectorAll("button")) {
        other.setAttribute("aria-pressed", String(other === button));
      }
    });
  }
}

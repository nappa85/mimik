// Closes the header menus on an outside click or Escape and keeps one open at a time.
// Without this script the <details> menus still open and close on their own.
(() => {
  const menus = () => document.querySelectorAll("header.site details[open]");
  document.addEventListener("click", (event) => {
    for (const menu of menus()) {
      if (!menu.contains(event.target)) menu.open = false;
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    for (const menu of menus()) {
      menu.open = false;
      menu.querySelector("summary")?.focus();
    }
  });
  document.addEventListener(
    "toggle",
    (event) => {
      if (!event.target.open || !event.target.matches("header.site details")) return;
      for (const menu of menus()) {
        if (menu !== event.target) menu.open = false;
      }
    },
    true,
  );
})();

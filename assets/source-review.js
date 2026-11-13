// Progressive enhancement only: all decisions remain authenticated server forms.
(() => {
  const list = document.querySelector('#review-list');
  // A refused save: the server renders the error next to the field; bring it into view and focus it.
  document.querySelector('[data-save-error]')?.focus();
  const toggle = document.querySelector('#review-shortcuts');
  if (toggle) {
    toggle.closest('label').hidden = false;
    try { toggle.checked = localStorage.getItem('source-review-shortcuts') !== 'off'; } catch (_) { /* Storage is optional. */ }
    toggle.addEventListener('change', () => {
      try { localStorage.setItem('source-review-shortcuts', toggle.checked ? 'on' : 'off'); } catch (_) { /* Storage is optional. */ }
    });
  }
  list?.addEventListener('keydown', event => {
    // j / k also work on a card's Accept button (so `a` then `j` moves on); other keys stay its own.
    const onAccept = event.target.matches('[data-accept-unit]') && (event.key === 'j' || event.key === 'k');
    if (!toggle?.checked || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey ||
        (!onAccept && event.target.closest('input,textarea,select,button,[contenteditable]'))) return;
    const card = event.target.closest('[data-unit]');
    if (!card) return;
    const cards = [...list.querySelectorAll('[data-unit]')];
    const index = cards.indexOf(card);
    let target;
    if (event.key === 'j') target = cards[index + 1];
    if (event.key === 'k') target = cards[index - 1];
    if (event.key === 'a') target = card.querySelector('[data-accept-unit]:not(:disabled)');
    if (target) { event.preventDefault(); target.focus(); }
  });
  // A link into a closed disclosure (page task, details) opens it, so the target is visible.
  const reveal = (hash = location.hash) => {
    let target = null;
    try { target = document.getElementById(decodeURIComponent(hash.slice(1))); } catch (_) { /* Bad escape: nothing to open. */ }
    for (let node = target?.parentElement?.closest('details'); node; node = node.parentElement?.closest('details')) node.open = true;
  };
  window.addEventListener('hashchange', () => reveal());
  // A same-page link whose hash is already current fires no hashchange: open on click as well.
  document.addEventListener('click', event => {
    const link = event.target.closest?.('a[href*="#"]');
    if (!link) return;
    const url = new URL(link.href, location.href);
    if (url.origin === location.origin && url.pathname === location.pathname && url.search === location.search) reveal(url.hash);
  });
  reveal();
  // Create-table bar: appears at the bottom of the viewport while units are ticked. The plain
  // buttons stay for browsers without script.
  const bar = document.querySelector('#table-bar');
  if (bar) {
    const ticks = () => [...document.querySelectorAll('input[name=unit][form=table-select]')];
    const count = bar.querySelector('[data-selected-count]');
    const announce = document.querySelector('[data-selected-announce]');
    document.querySelectorAll('[data-no-script-table]').forEach(button => { button.hidden = true; });
    const refresh = () => {
      const n = ticks().filter(box => box.checked).length;
      bar.hidden = n === 0;
      // The bar must not cover the content: pad the page while it shows.
      document.documentElement.classList.toggle('has-table-bar', n > 0);
      const message = (n === 1 ? '1 unit selected' : `${n} units selected`) + (n > 200 ? ' (at most 200)' : '');
      count.textContent = message;
      if (announce) announce.textContent = n === 0 ? '' : message;
    };
    ticks().forEach(box => box.addEventListener('change', refresh));
    bar.querySelector('[data-clear-selection]').addEventListener('click', () => {
      ticks().forEach(box => { box.checked = false; });
      refresh();
    });
    refresh();
  }
  const image = document.querySelector('#review-image');
  const svg = document.querySelector('#review-overlay');
  if (!image || !svg) return;
  document.querySelector('.review-zoom-controls').hidden = false;
  document.querySelectorAll('[data-zoom]').forEach(button => button.addEventListener('click', () => {
    document.querySelectorAll('[data-zoom]').forEach(control => control.setAttribute('aria-pressed', String(control === button)));
    image.classList.remove('zoom-one', 'zoom-two');
    const original = image.querySelector('img');
    // CSSOM, not a width attribute: `.source-original img` sets a CSS width, which beats an
    // attribute (seen in the part-3 browser check), and the policy allows CSSOM styles.
    original.style.width = button.dataset.zoom === 'two' && original.naturalWidth ? `${original.naturalWidth * 2}px` : '';
    if (button.dataset.zoom !== 'fit') image.classList.add(`zoom-${button.dataset.zoom}`);
  }));
  list?.addEventListener('focusin', event => {
    const card = event.target.closest('[data-unit]');
    document.querySelectorAll('[data-overlay-unit]').forEach(box => box.classList.toggle('highlight', box.dataset.overlayUnit === card?.dataset.unit));
  });
  document.querySelectorAll('[data-suggestion]').forEach(suggestion => {
    suggestion.addEventListener('pointerenter', () => highlight(suggestion, true));
    suggestion.addEventListener('pointerleave', () => highlight(suggestion, false));
    suggestion.addEventListener('focusin', () => highlight(suggestion, true));
    suggestion.addEventListener('focusout', () => highlight(suggestion, false));
  });
  function highlight(suggestion, enabled) {
    const ids = (suggestion.dataset.targets || '').split(',');
    document.querySelectorAll('[data-overlay-unit]').forEach(box => {
      if (ids.includes(box.dataset.overlayUnit)) box.classList.toggle('highlight', enabled);
    });
  }
})();

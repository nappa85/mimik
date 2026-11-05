// Progressive enhancement only: all decisions remain authenticated server forms.
(() => {
  const list = document.querySelector('#review-list');
  const toggle = document.querySelector('#review-shortcuts');
  if (toggle) {
    toggle.closest('label').hidden = false;
    try { toggle.checked = localStorage.getItem('source-review-shortcuts') !== 'off'; } catch (_) { /* Storage is optional. */ }
    toggle.addEventListener('change', () => {
      try { localStorage.setItem('source-review-shortcuts', toggle.checked ? 'on' : 'off'); } catch (_) { /* Storage is optional. */ }
    });
  }
  list?.addEventListener('keydown', event => {
    if (!toggle?.checked || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey ||
        event.target.closest('input,textarea,select,button,[contenteditable]')) return;
    const card = event.target.closest('[data-unit]');
    if (!card) return;
    const cards = [...list.querySelectorAll('[data-unit]')];
    const index = cards.indexOf(card);
    let target;
    if (event.key === 'j') target = cards[index + 1];
    if (event.key === 'k') target = cards[index - 1];
    if (event.key === 'a') target = card.querySelector('[data-accept-rationale]:not(:disabled)');
    if (target) { event.preventDefault(); target.focus(); }
  });
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

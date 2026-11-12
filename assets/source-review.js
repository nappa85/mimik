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
  // Decision dropdown: without script every field shows; hide the ones the chosen decision does
  // not use (accepted: none, corrected: text and rationale, rejected: rationale).
  document.querySelectorAll('select[name=state][form]').forEach(select => {
    const form = document.getElementById(select.getAttribute('form'));
    const fields = form ? [...form.querySelectorAll('[data-decision-fields]')] : [];
    const apply = () => fields.forEach(field => {
      field.hidden = !field.dataset.decisionFields.split(' ').includes(select.value);
    });
    select.addEventListener('change', apply);
    apply();
  });
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
      const message = (n === 1 ? '1 unit selected' : `${n} units selected`) + (n > 200 ? ' (at most 200 per table)' : '');
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

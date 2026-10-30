// Progressive enhancement only: all decisions remain authenticated server forms.
(() => {
  const list = document.querySelector('#review-list');
  const toggle = document.querySelector('#review-shortcuts');
  if (toggle) {
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
    image.classList.remove('zoom-one', 'zoom-two');
    const original = image.querySelector('img');
    original.removeAttribute('width');
    if (button.dataset.zoom === 'two' && original.naturalWidth) original.setAttribute('width', String(original.naturalWidth * 2));
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
    const ids = [...(suggestion.dataset.targets || '').matchAll(/unit (\d+)/g)].map(match => match[1]);
    document.querySelectorAll('[data-overlay-unit]').forEach(box => {
      if (ids.includes(box.dataset.overlayUnit)) box.classList.toggle('highlight', enabled);
    });
  }
  const draw = document.querySelector('#draw-region');
  const input = document.querySelector('#owner-polygon');
  const rectangle = document.querySelector('#drawn-region');
  if (!draw || !input || !rectangle) return;
  draw.hidden = false;
  let drawing = false;
  let start = null;
  let corners = null;
  function position(event) {
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(svg.getScreenCTM().inverse());
    return [Math.max(0, Math.min(Number(image.dataset.width), point.x)), Math.max(0, Math.min(Number(image.dataset.height), point.y))];
  }
  function update(a, b) {
    const x = Math.min(a[0], b[0]), y = Math.min(a[1], b[1]);
    const right = Math.max(a[0], b[0]), bottom = Math.max(a[1], b[1]);
    corners = [[x,y],[right,y],[right,bottom],[x,bottom]];
    input.value = JSON.stringify(corners);
    rectangle.hidden = false;
    for (const [key,value] of Object.entries({x,y,width:right-x,height:bottom-y})) rectangle.setAttribute(key, String(value));
    document.querySelector('#region-handles').hidden = false;
  }
  draw.addEventListener('click', () => {
    drawing = !drawing;
    draw.setAttribute('aria-pressed', String(drawing));
    svg.classList.toggle('drawing', drawing);
  });
  svg.addEventListener('pointerdown', event => {
    if (!drawing) return;
    event.preventDefault(); start = position(event); svg.setPointerCapture(event.pointerId);
  });
  svg.addEventListener('pointermove', event => { if (start) update(start, position(event)); });
  svg.addEventListener('pointerup', event => {
    if (!start) return;
    update(start, position(event)); start = null; drawing = false; svg.classList.remove('drawing'); draw.setAttribute('aria-pressed', 'false');
  });
  svg.addEventListener('click', event => { if (drawing) event.preventDefault(); });
  document.querySelectorAll('[data-corner]').forEach(handle => handle.addEventListener('keydown', event => {
    if (!corners || !['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)) return;
    event.preventDefault();
    const index = Number(handle.dataset.corner), other = index === 0 ? 2 : 0;
    const p = [...corners[index]], step = event.shiftKey ? 10 : 1;
    if (event.key === 'ArrowLeft') p[0] -= step;
    if (event.key === 'ArrowRight') p[0] += step;
    if (event.key === 'ArrowUp') p[1] -= step;
    if (event.key === 'ArrowDown') p[1] += step;
    p[0] = Math.max(0, Math.min(Number(image.dataset.width), p[0]));
    p[1] = Math.max(0, Math.min(Number(image.dataset.height), p[1]));
    update(p, corners[other]);
  }));
})();

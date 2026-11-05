// Bounded recovery for original-region previews when the local crop worker is busy.
(() => {
  const states = new WeakMap();
  let sequence = 0;
  const region = image => image instanceof HTMLImageElement && image.closest('.source-region');
  function stateFor(image) {
    if (!states.has(image)) {
      states.set(image, { attempts: 0, timer: null, loaded: false, url: image.src, stagger: (sequence++ % 4) * 100 });
    }
    return states.get(image);
  }
  function status(image, message) {
    const caption = image.closest('.source-region').querySelector('figcaption');
    let text = caption.querySelector('.region-status');
    if (!text) {
      text = document.createElement('span');
      text.className = 'region-status';
      text.setAttribute('role', 'status');
      caption.prepend(text);
    }
    text.textContent = message + ' ';
  }
  function failed(image) {
    if (!region(image)) return;
    const state = stateFor(image);
    if (state.loaded || state.timer !== null) return;
    if (state.attempts === 4) {
      status(image, 'Region preview unavailable; inspect the full-page image or open the region link for details.');
      return;
    }
    status(image, 'Retrying region preview…');
    const delay = 500 * (2 ** state.attempts) + state.stagger;
    state.timer = setTimeout(() => {
      state.timer = null;
      if (state.loaded || !image.isConnected) return;
      state.attempts += 1;
      const url = new URL(state.url);
      url.searchParams.set('region-retry', String(state.attempts));
      image.src = url.href;
    }, delay);
  }
  document.addEventListener('error', event => failed(event.target), true);
  document.addEventListener('load', event => {
    const image = event.target;
    if (!region(image)) return;
    const state = stateFor(image);
    state.loaded = true;
    if (state.timer !== null) clearTimeout(state.timer);
    state.timer = null;
    status(image, 'Region preview loaded.');
  }, true);
  // Observe only near-visible native lazy images. An unrequested lazy image has
  // no currentSrc; never turn the initial DOM sweep into eager image loading.
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (entry.isIntersecting) {
        const image = entry.target;
        if (image.currentSrc && image.complete && image.naturalWidth === 0) failed(image);
        observer.unobserve(image);
      }
    }
  });
  document.querySelectorAll('.source-region img').forEach(image => observer.observe(image));
})();

// One displayed-point editor for source review polygons and staged exclusion boxes.
// The server forms remain authoritative; all geometry travels as numeric values.
(() => {
  for (const root of document.querySelectorAll('[data-region-editor]')) {
    const svg = root.querySelector('.region-overlay, #review-overlay');
    const draw = root.querySelector('[data-region-draw], #draw-region');
    const keyboard = root.querySelector('[data-region-keyboard], #keyboard-region');
    const rectangle = root.querySelector('[data-region-rectangle], #drawn-region');
    const handles = root.querySelector('[data-region-handles], #region-handles');
    const polygon = root.querySelector('#owner-polygon');
    const fields = [...root.querySelectorAll('[data-coordinate]')];
    if (!svg || !draw || !keyboard || !rectangle || !handles) continue;
    const width = Number(root.dataset.width), height = Number(root.dataset.height);
    let drawing = false, start = null, corners = null;
    const selected = root.querySelector('input[type="checkbox"]');
    const extension = root.closest('form')?.querySelector('[name="extend_absent"]');
    const grips = [0, 2].map(corner => {
      const grip = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
      grip.classList.add('region-drag-handle');
      grip.dataset.regionCorner = String(corner);
      grip.setAttribute('r', '3');
      grip.setAttribute('hidden', '');
      svg.append(grip);
      return grip;
    });
    function visibility() {
      if (root.dataset.absent === 'true') {
        rectangle.toggleAttribute('hidden', !selected?.checked || !extension?.checked);
      } else if (selected) {
        rectangle.toggleAttribute('hidden', !selected.checked);
      }
      for (const grip of grips) grip.toggleAttribute('hidden', rectangle.hasAttribute('hidden'));
    }
    const clamp = p => [Math.max(0, Math.min(width, p[0])), Math.max(0, Math.min(height, p[1]))];
    function position(event) {
      const matrix = svg.getScreenCTM();
      if (!matrix) return null;
      const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
      return clamp([point.x, point.y]);
    }
    function update(a, b) {
      const x = Math.min(a[0], b[0]), y = Math.min(a[1], b[1]);
      const right = Math.max(a[0], b[0]), bottom = Math.max(a[1], b[1]);
      corners = [[x,y],[right,y],[right,bottom],[x,bottom]];
      if (polygon) polygon.value = JSON.stringify(corners);
      for (const field of fields) field.value = String([x,y,right,bottom][Number(field.dataset.coordinate)]);
      rectangle.removeAttribute('hidden');
      grips.forEach((grip, i) => {
        const corner = corners[i === 0 ? 0 : 2];
        grip.setAttribute('cx', String(corner[0]));
        grip.setAttribute('cy', String(corner[1]));
      });
      visibility();
      if (rectangle.tagName.toLowerCase() === 'polygon') {
        rectangle.setAttribute('points', corners.map(p => p.join(',')).join(' '));
      } else {
        for (const [key,value] of Object.entries({x,y,width:right-x,height:bottom-y})) rectangle.setAttribute(key, String(value));
      }
      handles.hidden = false;
    }
    function entered() {
      if (fields.length === 4) {
        const values = fields.map(f => f.value === '' ? NaN : Number(f.value));
        return values.every(Number.isFinite) ? [[values[0],values[1]], null, [values[2],values[3]]] : null;
      }
      let value;
      try { value = JSON.parse(polygon.value); } catch (_) { return null; }
      return Array.isArray(value) && value.length === 4 && value.every(p => Array.isArray(p) && p.length === 2 && p.every(Number.isFinite)) ? value : null;
    }
    function preview() {
      const value = entered();
      if (!value) return;
      // Preserve an invalid/submitted draft exactly; previewing must not rewrite it.
      const saved = polygon?.value, numbers = fields.map(f => f.value);
      update(value[0], value[2]);
      if (polygon) polygon.value = saved;
      fields.forEach((f, i) => { f.value = numbers[i]; });
    }
    preview();
    selected?.addEventListener('change', visibility);
    extension?.addEventListener('change', visibility);
    for (const field of fields) field.addEventListener('input', preview);
    draw.hidden = false;
    keyboard.hidden = false;
    keyboard.addEventListener('click', () => {
      const value = entered();
      update(value ? value[0] : [width/4,height/4], value ? value[2] : [width/2,height/2]);
      root.querySelector('[data-corner="0"]').focus();
    });
    function stop() {
      start = null; drawing = false;
      svg.classList.remove('drawing'); draw.setAttribute('aria-pressed', 'false');
    }
    draw.addEventListener('click', () => {
      drawing = !drawing; start = null;
      draw.setAttribute('aria-pressed', String(drawing)); svg.classList.toggle('drawing', drawing);
    });
    svg.addEventListener('pointerdown', event => {
      const grip = event.target.closest('[data-region-corner]');
      if (grip && corners) {
        start = corners[grip.dataset.regionCorner === '0' ? 2 : 0];
      } else if (drawing) {
        start = position(event);
      } else return;
      event.preventDefault(); svg.setPointerCapture(event.pointerId);
    });
    svg.addEventListener('pointermove', event => {
      const p = position(event); if (start && p) update(start, p);
    });
    svg.addEventListener('pointerup', event => {
      const p = position(event); if (start && p) update(start, p); stop();
    });
    svg.addEventListener('pointercancel', stop);
    svg.addEventListener('lostpointercapture', stop);
    svg.addEventListener('click', event => { if (drawing) event.preventDefault(); });
    for (const handle of root.querySelectorAll('[data-corner]')) {
      handle.addEventListener('keydown', event => {
        if (!corners || !['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        const index = Number(handle.dataset.corner), other = index === 0 ? 2 : 0;
        const p = [...corners[index]], step = event.shiftKey ? 10 : 1;
        if (event.key === 'ArrowLeft') p[0] -= step;
        if (event.key === 'ArrowRight') p[0] += step;
        if (event.key === 'ArrowUp') p[1] -= step;
        if (event.key === 'ArrowDown') p[1] += step;
        update(clamp(p), corners[other]);
      });
    }
  }
})();

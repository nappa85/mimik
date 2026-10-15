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

// Mimik acknowledgement module (CAMP-14, owner decision 1; docs/assisted-entry.md, "Opened
// acknowledgement"). A current Master's sheet page that offers the one-click form
// (`#seen-form`, carrying the exact rendered revision) submits that same form once, in the
// background, as soon as the page is visible: the Master has opened it. Without this module
// (JavaScript off, blocked or failing) the form stays one click.
//
// The module posts the form's own fields to its own action and nothing else: the revision is
// the one the server rendered, never one read from a later answer, so a draft, a refused save
// or another Master's revision cannot be acknowledged by it. It builds DOM with createElement
// and textContent only and runs under the strict CSP (no inline script or styling).

const ACTION = /^\/private-sheets\/\d+\/seen$/;
const DIGITS = /^\d+$/;

function acknowledge(form) {
  const csrf = form.elements.namedItem('csrf');
  const revision = form.elements.namedItem('revision');
  const action = form.getAttribute('action');
  if (!csrf || !revision || !ACTION.test(action) || !DIGITS.test(revision.value)) return;
  const body = new URLSearchParams();
  body.set('csrf', csrf.value);
  body.set('revision', revision.value);
  const shown = revision.value;
  // `redirect: 'manual'`: the 303 to the sheet is not followed (the page is already it).
  fetch(action, {
    method: 'POST',
    body,
    credentials: 'same-origin',
    redirect: 'manual',
    cache: 'no-store',
  }).then((response) => {
    if (response.type !== 'opaqueredirect' && response.status !== 303) return;
    const state = document.createElement('p');
    state.id = 'seen-state';
    state.textContent = `Opened (revision ${shown})`;
    form.replaceWith(state);
  }, () => {});
}

function start() {
  const form = document.getElementById('seen-form');
  if (!form) return;
  let sent = false;
  const go = () => {
    if (sent || document.visibilityState !== 'visible') return;
    sent = true;
    document.removeEventListener('visibilitychange', go);
    acknowledge(form);
  };
  document.addEventListener('visibilitychange', go);
  go();
}

start();

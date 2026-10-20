// Mimik sheet module (assisted sheet step 2a; docs/assisted-entry.md, "Script on sheet
// pages"). Progressive enhancement of a saved runtime sheet the reader may edit: Name,
// General notes, each scalar field and each row cell save through their own route without
// reloading the page, on Enter or Save and when the player leaves a changed field (D-AA).
// Without this module every save keeps working through POST/redirect/GET (D-G).
//
// The page runs under a strict CSP with Trusted Types and no policy: this module builds DOM
// with createElement and textContent only, and never sets inline styles.
//
// Per unit: one request in flight, the latest entry queued; `base` moves only for the saved
// unit and only to a newer revision; a draft generation counter keeps what the player typed
// while a save was on its way. Derived values, checks and the validity flag have their own
// projection revision and change only for an answer at least as new.

const UNIT_PATH = /^\/private-sheets\/\d+\/(?:meta\/(?:name|notes)|fields\/[^/]+|cells\/\d+\/\d+\/\d+)$/;
const DIGITS = /^\d+$/;
const JSON_TYPE = 'application/json';

/** Unit state by form. */
const units = new Map();
let projection = null;
let pending = 0;
let waiters = [];
const released = new WeakSet();
const waiting = new WeakSet();

function element(tag, attributes, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes || {})) {
    node.setAttribute(name, value);
  }
  for (const child of children) {
    node.append(child);
  }
  return node;
}

function canonical(pairs) {
  pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  return JSON.stringify(pairs);
}

/** A unit form's entry, without csrf and base, in a canonical order. */
function entryKey(form) {
  const pairs = [];
  for (const [name, value] of new FormData(form)) {
    if (name !== 'csrf' && name !== 'base') {
      pairs.push([name, typeof value === 'string' ? value : '']);
    }
  }
  return canonical(pairs);
}

/** The entry key of what the server answered for the unit. */
function answerKey(unit, shown) {
  const pairs = [['value', shown.value ?? '']];
  if (unit.state) {
    pairs.push([unit.state.name, shown.state ?? '']);
  }
  return canonical(pairs);
}

function announce(text) {
  const region = document.querySelector('main [role="status"]');
  if (region) {
    region.textContent = text;
  }
}

function whenIdle() {
  if (pending === 0) {
    return Promise.resolve();
  }
  return new Promise((resolve) => waiters.push(resolve));
}

function settled() {
  if (pending === 0) {
    const ready = waiters;
    waiters = [];
    for (const resolve of ready) {
      resolve();
    }
  }
}

/** The DOM id suffix of a unit's note and actions, and the ids that name its buttons. */
function names(unit) {
  const id = unit.control.id;
  const field = /^v(\d+)$/.exec(id);
  if (field) {
    return { key: field[1], labelled: `label-${field[1]}` };
  }
  const cell = /^row-((\d+)-(\d+)-\d+)$/.exec(id);
  if (cell) {
    return {
      key: cell[1],
      labelled: `label-${cell[1]} list-legend-${cell[2]} legend-${cell[2]}-${cell[3]}`,
    };
  }
  return null;
}

function saveButton(form) {
  return form.querySelector('button:not([form])');
}

function setAlert(unit, text) {
  let alert = unit.form.querySelector('.entry-alert');
  if (!alert) {
    alert = element('p', { class: 'entry-alert', role: 'alert' });
    saveButton(unit.form).after(alert);
  }
  alert.textContent = text;
}

function clearAlert(unit) {
  unit.form.querySelector('.entry-alert')?.remove();
}

/** An integer control: the page marks an empty one unknown with `placeholder="?"` and the
 * hidden "Unknown value" hint. */
function integer(control) {
  return control instanceof HTMLInputElement && !control.hasAttribute('maxlength');
}

function describe(unit, ids) {
  const control = unit.control;
  const list = [];
  if (integer(control) && control.value === '') {
    list.push('unknown-hint');
    control.setAttribute('placeholder', '?');
  } else if (integer(control)) {
    control.removeAttribute('placeholder');
  }
  list.push(...ids);
  if (list.length) {
    control.setAttribute('aria-describedby', list.join(' '));
  } else {
    control.removeAttribute('aria-describedby');
  }
}

function showNote(unit, note, at) {
  const shown = names(unit);
  unit.form.querySelector('.entry-note')?.remove();
  if (!shown || !note) {
    return [];
  }
  const line = element('p', { class: 'entry-note', id: `note-${shown.key}` });
  if (note.kind === 'sum') {
    line.append(element('bdi', {}, note.shown), ` = ${note.value}`);
  } else {
    line.append(
      note.unknown ? '? (unknown)' : `kept ${note.value}`,
      ' · ‘',
      element('bdi', {}, note.shown),
      '’ not understood',
    );
  }
  at.after(line);
  return [line.id];
}

/** A hidden form of its own for the unit's switch or Undo, carrying the answer's revision
 * (the page's shared forms keep the revision the page was loaded at). */
function actionForm(unit, verb, key, revision) {
  const id = `assist-${verb}-${key}`;
  document.getElementById(id)?.remove();
  const action = new URL(unit.form.action).pathname.replace(
    /\/(?:meta|fields|cells)\/.*$/,
    `/${verb}`,
  );
  const csrf = unit.form.elements.namedItem('csrf').value;
  const form = element(
    'form',
    { id, method: 'post', action, hidden: '' },
    element('input', { type: 'hidden', name: 'csrf', value: csrf }),
    element('input', { type: 'hidden', name: 'base', value: revision }),
  );
  document.querySelector('main').append(form);
  return id;
}

function showActions(unit, actions, revision, at) {
  const shown = names(unit);
  unit.form.querySelector('.entry-actions')?.remove();
  if (!shown || !actions || (!actions.offer && actions.undo === null)) {
    return [];
  }
  const { key, labelled } = shown;
  const box = element('div', { class: 'entry-actions' });
  const ids = [];
  if (actions.offer) {
    const form = actionForm(unit, 'switch', key, revision);
    const body = element(
      'p',
      { class: 'offer-body', id: `offer-${key}` },
      'Did you mean ',
      element('bdi', {}, actions.offer.text),
      ` = ${actions.offer.value}? `,
      element(
        'button',
        {
          type: 'submit',
          form,
          name: 'note',
          value: actions.note,
          id: `switch-${key}`,
          'aria-labelledby': `switch-${key} ${labelled}`,
        },
        'Yes',
      ),
      element('label', { for: `dismiss-${key}` }, 'Hide'),
    );
    box.append(
      element(
        'div',
        { class: 'entry-offer' },
        element('input', {
          type: 'checkbox',
          class: 'offer-dismiss',
          id: `dismiss-${key}`,
          'aria-label': 'Hide the suggestion',
        }),
        body,
      ),
    );
    ids.push(body.id);
  }
  if (actions.undo !== null) {
    const form = actionForm(unit, 'undo', key, revision);
    box.append(
      element(
        'p',
        { class: 'entry-undo' },
        element('span', { id: `was-${key}` }, `Last change: was ${actions.undo}`),
        element(
          'button',
          {
            type: 'submit',
            form,
            name: 'note',
            value: actions.note,
            id: `undo-${key}`,
            'aria-labelledby': `undo-${key} ${labelled}`,
            'aria-describedby': `was-${key}`,
          },
          'Undo',
        ),
      ),
    );
  }
  at.after(box);
  return ids;
}

/** Puts the saved unit on the page: its control (only when `replace`: the draft is the
 * one sent), its note, switch offer and Undo. */
function showUnit(unit, shown, revision, replace) {
  if (replace) {
    unit.control.value = shown.value ?? '';
    if (unit.state && typeof shown.state === 'string') {
      unit.state.value = shown.state;
    }
  }
  const button = saveButton(unit.form);
  const noteIds = showNote(unit, shown.note, button);
  const after = unit.form.querySelector('.entry-note') || button;
  const offerIds = showActions(unit, shown.actions, revision, after);
  describe(unit, [...noteIds, ...offerIds]);
}

function setText(id, text) {
  const node = document.getElementById(id);
  if (node) {
    node.textContent = text;
  }
}

function showValidity(validity, revision) {
  const old = document.getElementById('validity');
  if (validity.validated) {
    old?.remove();
    return;
  }
  const flag = element(
    'p',
    { id: 'validity', class: 'validity' },
    element('strong', {}, 'Not validated'),
    ` (saved revision ${revision}): `,
  );
  validity.reasons.forEach((reason, index) => {
    if (index > 0) {
      flag.append('; ');
    }
    flag.append(
      reason.anchor ? element('a', { href: `#${reason.anchor}` }, reason.text) : reason.text,
    );
  });
  flag.append('. This flag never blocks saving.');
  if (old) {
    old.replaceWith(flag);
  } else {
    document.querySelector('main > h1')?.nextElementSibling?.after(flag);
  }
}

function checkItem(check) {
  const item = element('li', {}, element('strong', {}, check.word), `: ${check.message}`);
  if (check.code !== null) {
    item.append(` Reason: ${check.explanation}. `, element('code', {}, check.code));
  }
  item.append(
    element(
      'details',
      {},
      element('summary', {}, 'How this is computed'),
      element('pre', { class: 'evidence-text' }, check.expr),
      element('p', {}, `Check ${check.id} · Rule IDs: ${check.rule_ids}`),
    ),
  );
  return item;
}

function showChecks(section, checks) {
  const counts = `${checks.violated} violated, ${checks.not_evaluated} not evaluated`;
  setText('checks-counts', `${counts}, ${checks.passed} passed.`);
  setText('checks-link', `Advisory checks: ${counts}`);
  for (const old of section.querySelectorAll(':scope > ul, :scope > details')) {
    old.remove();
  }
  const attention = element('ul', {});
  const passing = element('ul', {});
  for (const check of checks.items) {
    (check.status === 'pass' ? passing : attention).append(checkItem(check));
  }
  section.append(attention);
  if (checks.passed !== '0') {
    section.append(
      element('details', {}, element('summary', {}, `Passing checks (${checks.passed})`), passing),
    );
  }
}

function showDerived(answer) {
  for (const item of answer.derived) {
    const value = document.getElementById(`label-${item.id}`)?.nextElementSibling;
    if (!value) {
      continue;
    }
    if (item.code === null) {
      value.replaceChildren(item.text);
    } else {
      value.replaceChildren(`Not computed: ${item.text}. `, element('code', {}, item.code));
    }
  }
  setText('derived-revision', answer.revision);
  document.getElementById('not-computed')?.remove();
  const missing = answer.not_computed;
  const anchor = document.getElementById('derived-revision')?.parentElement;
  if (missing && anchor) {
    anchor.after(
      element(
        'p',
        { id: 'not-computed' },
        `${missing.text}: each shows “Not computed” and its reason next to its field, starting with `,
        element('a', { href: `#label-${missing.id}` }, missing.label),
        '.',
      ),
    );
  }
}

/** Derived values, checks and the validity flag, from an answer at least as new as the
 * ones shown. */
function project(answer, revision) {
  if (projection !== null && revision < projection) {
    return;
  }
  projection = revision;
  showValidity(answer.validity, answer.revision);
  const section = document.getElementById('advisory-checks');
  const results = document.getElementById('formula-results');
  const shaped = answer.unavailable === null && (answer.checks === null) === (section === null);
  document.getElementById('projection-stale')?.remove();
  if (!shaped) {
    // The sections on the page no longer fit (the engine now refuses, or accepts, the
    // saved values): say so instead of showing stale results.
    (results || document.querySelector('main')).append(
      element(
        'p',
        { id: 'projection-stale', class: 'form-error', role: 'alert' },
        `Derived values and checks changed with saved revision ${answer.revision}; reload the page to see them.`,
      ),
    );
    return;
  }
  if (results) {
    showDerived(answer);
  }
  if (section && answer.checks) {
    showChecks(section, answer.checks);
  }
}

function failure(status) {
  if (status === 503) {
    return 'Not saved: the server is busy. Your entry is kept in its field; press Enter or Save to try again.';
  }
  if (status === 401 || status === 403) {
    return 'Not saved: this sheet can no longer be edited from this page, or the session ended. Your entry is kept in its field; reload the page.';
  }
  const code = status ? ` (HTTP ${status})` : '';
  return `Not saved${code}. Your entry is kept in its field; press Enter or Save to try again, or reload the page.`;
}

function valid(answer) {
  return (
    answer !== null &&
    typeof answer === 'object' &&
    typeof answer.revision === 'string' &&
    DIGITS.test(answer.revision) &&
    typeof answer.message === 'string' &&
    answer.unit !== null &&
    typeof answer.unit === 'object' &&
    Array.isArray(answer.derived) &&
    answer.validity !== null &&
    typeof answer.validity === 'object'
  );
}

function settle(unit, flight, answer, status, submitter) {
  if (flight.seq < unit.applied) {
    return;
  }
  unit.applied = flight.seq;
  const focusBack = submitter && document.activeElement === submitter;
  if (!valid(answer)) {
    setAlert(unit, failure(status));
    return;
  }
  const revision = BigInt(answer.revision);
  if (revision > unit.base) {
    unit.base = revision;
    unit.form.elements.namedItem('base').value = answer.revision;
    if (typeof answer.unit.action === 'string' && UNIT_PATH.test(answer.unit.action)) {
      unit.form.setAttribute('action', answer.unit.action);
    }
  }
  if (answer.status === 'saved' || answer.status === 'unchanged') {
    clearAlert(unit);
    unit.hold = false;
    const current = unit.gen === flight.gen;
    showUnit(unit, answer.unit, answer.revision, current && answer.status === 'saved');
    unit.ack = current ? entryKey(unit.form) : answerKey(unit, answer.unit);
    announce(answer.message);
    if (focusBack) {
      unit.control.focus();
    }
  } else {
    // Refused (409 or 422): the draft stays in its control; only Enter or Save sends it
    // again, so a conflict is overwritten consciously, never by leaving the field.
    unit.hold = true;
    setAlert(unit, answer.message);
    if (focusBack || document.activeElement === document.body) {
      unit.control.focus();
    }
  }
  project(answer, revision);
}

async function send(unit, key, submitter) {
  const body = new URLSearchParams();
  for (const [name, value] of new FormData(unit.form)) {
    body.append(name, typeof value === 'string' ? value : '');
  }
  body.set('base', unit.base.toString());
  unit.seq += 1;
  const flight = { key, gen: unit.gen, seq: unit.seq };
  unit.flight = flight;
  unit.sent = key;
  pending += 1;
  unit.form.setAttribute('aria-busy', 'true');
  let answer = null;
  let status = 0;
  try {
    const response = await fetch(unit.form.action, {
      method: 'POST',
      body,
      headers: { Accept: JSON_TYPE },
      credentials: 'same-origin',
      redirect: 'manual',
      cache: 'no-store',
    });
    status = response.status;
    const type = (response.headers.get('Content-Type') || '').split(';')[0].trim();
    if (type === JSON_TYPE) {
      answer = await response.json();
    }
  } catch {
    answer = null;
  }
  unit.flight = null;
  unit.form.removeAttribute('aria-busy');
  settle(unit, flight, answer, status, submitter);
  pending -= 1;
  const queued = unit.queued;
  if (queued) {
    unit.queued = null;
    pending -= 1;
    request(unit, queued.explicit, null);
  }
  settled();
}

/** Saves the unit: always on Enter or Save (`explicit`); on leaving or changing a control
 * only when the entry differs from the last one sent and the last one acknowledged. */
function request(unit, explicit, submitter) {
  const key = entryKey(unit.form);
  if (unit.flight) {
    if (key === unit.flight.key) {
      return;
    }
    if (unit.queued) {
      unit.queued.explicit ||= explicit;
    } else {
      unit.queued = { explicit };
      pending += 1;
    }
    return;
  }
  if (!explicit && (unit.hold || key === unit.sent || key === unit.ack)) {
    return;
  }
  send(unit, key, submitter);
}

function enhance(form) {
  if (form.method !== 'post') {
    return;
  }
  const path = new URL(form.action, document.baseURI).pathname;
  const base = form.elements.namedItem('base');
  const control = form.elements.namedItem('value');
  if (
    !UNIT_PATH.test(path) ||
    !(base instanceof HTMLInputElement) ||
    !DIGITS.test(base.value) ||
    !control ||
    control instanceof RadioNodeList
  ) {
    return;
  }
  const unit = {
    form,
    control,
    state: form.querySelector('select[name="null"], select[name="set"]'),
    base: BigInt(base.value),
    sent: null,
    ack: entryKey(form),
    gen: 0,
    flight: null,
    queued: null,
    seq: 0,
    applied: 0,
    hold: false,
  };
  units.set(form, unit);
  if (projection === null || unit.base > projection) {
    projection = unit.base;
  }
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    request(unit, true, event.submitter);
  });
  form.addEventListener('input', (event) => {
    if (event.target.name) {
      unit.gen += 1;
    }
  });
  form.addEventListener('change', (event) => {
    if (event.target instanceof HTMLSelectElement && event.target.name) {
      request(unit, false, null);
    }
  });
  form.addEventListener('focusout', (event) => {
    const target = event.target;
    const text =
      target instanceof HTMLTextAreaElement ||
      (target instanceof HTMLInputElement && target.type !== 'checkbox' && target.type !== 'hidden');
    if (text && target.name) {
      request(unit, false, null);
    }
  });
}

// Any other form (switch, Undo, row actions, quick add, new row) still posts and reloads the
// page: it waits until every save in flight or queued has its answer, so leaving a field
// for one of its buttons never loses the entry.
document.addEventListener('submit', (event) => {
  const form = event.target;
  if (!(form instanceof HTMLFormElement) || units.has(form)) {
    return;
  }
  if (released.has(form)) {
    released.delete(form);
    return;
  }
  if (pending === 0 && !waiting.has(form)) {
    return;
  }
  event.preventDefault();
  if (waiting.has(form)) {
    return;
  }
  waiting.add(form);
  const submitter = event.submitter && event.submitter.form === form ? event.submitter : null;
  whenIdle().then(() => {
    waiting.delete(form);
    released.add(form);
    form.requestSubmit(submitter);
  });
});

for (const form of document.forms) {
  enhance(form);
}
document.documentElement.setAttribute('data-sheet-assist', 'ready');

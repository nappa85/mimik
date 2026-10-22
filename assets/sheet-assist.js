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
const PROPOSAL_PATH = /^\/private-sheets\/\d+\/proposals\/[0-9a-f]{32}\/\d+$/;
const POLL_PATH = /^\/private-sheets\/\d+\/interpretations\/[0-9a-f]{32}$/;
const DIGITS = /^\d+$/;
const JSON_TYPE = 'application/json';

/** Unit state by form. */
const units = new Map();
let projection = null;
let pending = 0;
let waiters = [];
const released = new WeakSet();
const waiting = new WeakSet();
/** The assistant (step 2c-1): `false` once its start route answered 404 (it is off). */
let assistantOn = true;

/** The only attribute names `element` sets: no inline styling, no event handler, no URL that
 * loads anything. */
const ATTRIBUTES = new Set([
  'action', 'aria-describedby', 'aria-label', 'aria-labelledby', 'class', 'for', 'form',
  'hidden', 'href', 'id', 'method', 'name', 'role', 'type', 'value',
]);

function element(tag, attributes, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes || {})) {
    if (!ATTRIBUTES.has(name)) {
      throw new TypeError(`attribute ${name} is not allowed`);
    }
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

/** Shows `text` in the unit's alert, which describes its control; returns the alert's id. */
function setAlert(unit, text) {
  let alert = unit.form.querySelector('.entry-alert');
  if (!alert) {
    alert = element('p', { class: 'entry-alert', role: 'alert', id: `alert-${unit.control.id}` });
    const button = saveButton(unit.form);
    if (button) {
      button.after(alert);
    } else {
      unit.form.append(alert);
    }
  }
  alert.textContent = text;
  const ids = (unit.control.getAttribute('aria-describedby') || '').split(' ').filter(Boolean);
  if (!ids.includes(alert.id)) {
    unit.control.setAttribute('aria-describedby', [...ids, alert.id].join(' '));
  }
  return alert.id;
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

/** Removes the unit's note, switch offer and Undo, with the hidden forms they post. */
function clearActions(unit) {
  unit.form.querySelector('.entry-actions')?.remove();
  const shown = names(unit);
  if (shown) {
    document.getElementById(`assist-switch-${shown.key}`)?.remove();
    document.getElementById(`assist-undo-${shown.key}`)?.remove();
  }
}

function showActions(unit, actions, revision, at) {
  const shown = names(unit);
  clearActions(unit);
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
  // The page's own shape: a checks section exactly when the evaluation was available.
  const shaped = (answer.checks === null) === (section === null);
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

const text = (value) => typeof value === 'string';
const textOrNull = (value) => value === null || text(value);
const record = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const STATUSES = new Set(['saved', 'unchanged', 'conflict', 'invalid']);

function validUnit(unit, refused) {
  if (!record(unit) || !textOrNull(unit.action)) {
    return false;
  }
  if (refused || unit.action === null) {
    return unit.current === undefined || textOrNull(unit.current);
  }
  const note = unit.note;
  const actions = unit.actions;
  return (
    text(unit.value) &&
    textOrNull(unit.state) &&
    (note === null ||
      (record(note) && text(note.kind) && text(note.shown) && text(note.value))) &&
    (actions === null ||
      (record(actions) &&
        text(actions.note) &&
        textOrNull(actions.undo) &&
        (actions.offer === null ||
          (record(actions.offer) && text(actions.offer.text) && text(actions.offer.value)))))
  );
}

function validChecks(checks) {
  return (
    checks === null ||
    (record(checks) &&
      DIGITS.test(checks.violated) &&
      DIGITS.test(checks.not_evaluated) &&
      DIGITS.test(checks.passed) &&
      Array.isArray(checks.items) &&
      checks.items.every(
        (item) =>
          record(item) &&
          [item.id, item.status, item.word, item.message, item.expr, item.rule_ids].every(text) &&
          textOrNull(item.code) &&
          textOrNull(item.explanation),
      ))
  );
}

/** The answer has the shape the module draws from; anything else is a failure. */
function valid(answer) {
  if (!record(answer) || !STATUSES.has(answer.status)) {
    return false;
  }
  const refused = answer.status === 'conflict' || answer.status === 'invalid';
  const missing = answer.not_computed;
  const validity = answer.validity;
  return (
    text(answer.revision) &&
    DIGITS.test(answer.revision) &&
    text(answer.message) &&
    validUnit(answer.unit, refused) &&
    Array.isArray(answer.derived) &&
    answer.derived.every((item) => record(item) && text(item.id) && text(item.text) && textOrNull(item.code)) &&
    (missing === null || (record(missing) && text(missing.text) && text(missing.id) && text(missing.label))) &&
    validChecks(answer.checks) &&
    (answer.unavailable === null || record(answer.unavailable)) &&
    record(validity) &&
    typeof validity.validated === 'boolean' &&
    Array.isArray(validity.reasons) &&
    validity.reasons.every((reason) => record(reason) && text(reason.text) && textOrNull(reason.anchor))
  );
}

/** The unit has no route at the answer's revision (its row was removed): the entry stays
 * to copy and can no longer be posted, so a newer base is never paired with an old row
 * index (which would name another row). */
function orphan(unit, message) {
  // A queued save is dropped by `send`, which also releases its count in `pending`.
  unit.orphan = true;
  if (unit.control instanceof HTMLSelectElement) {
    // A select ignores `readonly`: disabled, it keeps showing the chosen option.
    unit.control.disabled = true;
  } else {
    unit.control.setAttribute('readonly', '');
  }
  if (unit.state) {
    unit.state.disabled = true;
  }
  const button = saveButton(unit.form);
  if (button) {
    button.disabled = true;
  }
  clearActions(unit);
  unit.form.querySelector('.entry-note')?.remove();
  describe(unit, []);
  setAlert(unit, message);
}

/** Applies an answer; returns whether a save queued meanwhile is dropped: after a
 * conflict or an orphaned unit (the player must see that first), not after a refused
 * value (the queued entry is a later one and goes). */
function settle(unit, flight, answer, status, submitter) {
  if (flight.seq < unit.applied) {
    return false;
  }
  unit.applied = flight.seq;
  const pressed = flight.submitter || submitter;
  const focusBack = pressed && document.activeElement === pressed;
  if (!valid(answer)) {
    setAlert(unit, failure(status));
    // A conflict whose body is unreadable is still a conflict: hold and drop the queue.
    if (status === 409) {
      unit.hold = true;
      return true;
    }
    return false;
  }
  const revision = BigInt(answer.revision);
  const refused = answer.status === 'conflict' || answer.status === 'invalid';
  const route =
    typeof answer.unit.action === 'string' && UNIT_PATH.test(answer.unit.action)
      ? answer.unit.action
      : null;
  if (route === null) {
    orphan(
      unit,
      refused
        ? answer.message
        : 'Saved, but its row was removed since; this entry is kept here to copy and can no longer be saved from this page. Reload the page to see the list.',
    );
    project(answer, revision);
    return true;
  }
  if (revision > unit.base) {
    unit.base = revision;
    unit.form.elements.namedItem('base').value = answer.revision;
    unit.form.setAttribute('action', route);
  }
  if (!refused) {
    clearAlert(unit);
    unit.hold = false;
    // What the server now holds and what the control shows are both unchanged; the raw
    // last request no longer is, so retyping "84 -15" after it was saved as 69 sends it
    // again, as Enter would.
    const current = unit.gen === flight.gen;
    showUnit(unit, answer.unit, answer.revision, current && answer.status === 'saved');
    interpret(unit, answer.unit.note);
    unit.sent = answerKey(unit, answer.unit);
    unit.ack = current ? entryKey(unit.form) : unit.sent;
    announce(answer.message);
    if (focusBack) {
      unit.control.focus();
    }
  } else {
    // The draft stays in its control. After a conflict only Enter or Save sends it again,
    // so it is overwritten consciously, never by leaving the field (ASSIST-23); after a
    // refused value a corrected entry saves as usual.
    unit.hold = answer.status === 'conflict';
    // As on the re-rendered page, a drafted unit shows no note, switch or Undo.
    unit.form.querySelector('.entry-note')?.remove();
    clearActions(unit);
    describe(unit, []);
    clearAlert(unit);
    const shown = typeof answer.unit.current === 'string' ? ` Saved value now: ${answer.unit.current}.` : '';
    setAlert(unit, `${answer.message}${shown}`);
    if (focusBack || document.activeElement === document.body) {
      unit.control.focus();
    }
  }
  project(answer, revision);
  return answer.status === 'conflict';
}

async function send(unit, key, submitter) {
  const body = new URLSearchParams();
  for (const [name, value] of new FormData(unit.form)) {
    body.append(name, typeof value === 'string' ? value : '');
  }
  body.set('base', unit.base.toString());
  unit.seq += 1;
  stopAssist(unit);
  const flight = { key, gen: unit.gen, seq: unit.seq, submitter };
  unit.flight = flight;
  unit.sent = key;
  pending += 1;
  unit.form.setAttribute('aria-busy', 'true');
  const { status, answer } = await bounded(unit.form.action, body);
  land(unit, () => settle(unit, flight, answer, status, submitter));
}

/** A POST asking for JSON, bounded: one that hangs, before its headers or inside its body,
 * counts as a network failure after 30 s, so a unit's flight, and page posts waiting for it,
 * never wait forever. Never throws. */
async function bounded(path, body) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 30000);
  try {
    const response = await fetch(path, {
      signal: controller.signal,
      method: 'POST',
      body,
      headers: { Accept: JSON_TYPE },
      credentials: 'same-origin',
      redirect: 'manual',
      cache: 'no-store',
    });
    const type = (response.headers.get('Content-Type') || '').split(';')[0].trim();
    return { status: response.status, answer: type === JSON_TYPE ? await response.json() : null };
  } catch {
    return { status: 0, answer: null };
  } finally {
    clearTimeout(timer);
  }
}

/** Ends a unit's flight (a save or a Use): `draw` shows the answer and says whether a save
 * queued meanwhile is dropped (after a conflict or an orphaned unit, ASSIST-23); otherwise it
 * is sent. The pending count is released whatever happens. */
function land(unit, draw) {
  unit.flight = null;
  unit.form.removeAttribute('aria-busy');
  let drop = false;
  try {
    drop = draw();
  } catch {
    setAlert(unit, failure(0));
  } finally {
    pending -= 1;
    const queued = unit.queued;
    unit.queued = null;
    if (queued) {
      pending -= 1;
    }
    try {
      // A save queued before a conflict was shown is dropped: the player sees the
      // conflict first, then saves again (ASSIST-23).
      if (queued && !drop) {
        request(unit, queued.explicit, queued.submitter);
      }
    } finally {
      settled();
    }
  }
}

/** Saves the unit: always on Enter or Save (`explicit`); on leaving or changing a control
 * only when the entry differs from the last one sent and the last one acknowledged. */
function request(unit, explicit, submitter) {
  if (unit.orphan) {
    return;
  }
  const key = entryKey(unit.form);
  if (unit.flight) {
    if (key === unit.flight.key) {
      // The same entry is on its way (left, then Save): its answer returns focus to the
      // cell for this press.
      unit.flight.submitter ||= submitter;
      return;
    }
    if (unit.queued) {
      unit.queued.explicit ||= explicit;
      unit.queued.submitter ||= submitter;
    } else {
      unit.queued = { explicit, submitter };
      pending += 1;
    }
    return;
  }
  if (!explicit && (unit.hold || key === unit.sent || key === unit.ack)) {
    return;
  }
  send(unit, key, submitter);
}

// The interpretation assistant (step 2c-1; docs/assisted-entry.md, "Assistant"). A saved
// scribble (`unresolved` note with a `key`) starts one job; "Thinking…" shows until the
// options, the readings of an ambiguous reply or a message arrive. Use posts the apply
// route; ignoring the options leaves the scribble. Model text is shown with textContent
// inside <bdi>, never as markup.

/** The sheet's path prefix (`/private-sheets/<id>`) of a unit. */
function sheetPath(unit) {
  const match = /^\/private-sheets\/\d+/.exec(new URL(unit.form.action).pathname);
  return match ? match[0] : null;
}

function assistBox(unit) {
  const shown = names(unit);
  if (!shown) {
    return null;
  }
  let box = document.getElementById(`assist-${shown.key}`);
  if (!box) {
    box = element('div', { class: 'entry-assist', id: `assist-${shown.key}`, role: 'status' });
    const after = unit.form.querySelector('.entry-actions') || unit.form.querySelector('.entry-note') || saveButton(unit.form);
    if (after) {
      after.after(box);
    } else {
      unit.form.append(box);
    }
    const ids = (unit.control.getAttribute('aria-describedby') || '').split(' ').filter(Boolean);
    if (!ids.includes(box.id)) {
      unit.control.setAttribute('aria-describedby', [...ids, box.id].join(' '));
    }
  }
  return box;
}

/** Ends the unit's job on this page: its polling stops and its box goes. */
function stopAssist(unit) {
  unit.assist += 1;
  const shown = names(unit);
  if (shown) {
    document.getElementById(`assist-${shown.key}`)?.remove();
  }
}

function assistText(unit, text) {
  const box = assistBox(unit);
  if (box) {
    box.replaceChildren(element('p', { class: 'assist-message' }, text));
  }
}

function csrfOf(unit) {
  const csrf = unit.form.elements.namedItem('csrf');
  return csrf instanceof HTMLInputElement ? csrf.value : '';
}

function postForm(path, pairs) {
  return bounded(path, new URLSearchParams(pairs));
}

/** Starts a job for a saved scribble and polls it. */
async function interpret(unit, note) {
  if (!assistantOn || !record(note) || note.kind !== 'unresolved' || !text(note.key)) {
    return;
  }
  const base = sheetPath(unit);
  if (!base) {
    return;
  }
  stopAssist(unit);
  const token = unit.assist;
  const started = await postForm(`${base}/interpretations`, [
    ['csrf', csrfOf(unit)],
    ['note', note.key],
  ]);
  if (token !== unit.assist) {
    return;
  }
  if (started.status === 404) {
    assistantOn = false;
    return;
  }
  const answer = started.answer;
  if ((started.status === 202 || started.status === 200) && record(answer) && text(answer.poll) && POLL_PATH.test(answer.poll)) {
    assistText(unit, 'Thinking\u2026');
    poll(unit, token, answer.poll, 500);
  } else if (record(answer) && text(answer.message) && started.status !== 409) {
    assistText(unit, answer.message);
  }
}

function poll(unit, token, path, delay) {
  setTimeout(async () => {
    if (token !== unit.assist || !document.contains(unit.form)) {
      return;
    }
    let status = 0;
    let answer = null;
    try {
      const response = await fetch(path, {
        headers: { Accept: JSON_TYPE },
        credentials: 'same-origin',
        cache: 'no-store',
      });
      status = response.status;
      const type = (response.headers.get('Content-Type') || '').split(';')[0].trim();
      answer = type === JSON_TYPE ? await response.json() : null;
    } catch {
      answer = null;
    }
    if (token !== unit.assist) {
      return;
    }
    if (status === 200 && record(answer) && answer.state === 'thinking') {
      poll(unit, token, path, Math.min(2000, Math.round(delay * 1.5)));
      return;
    }
    showAssistant(unit, token, answer);
  }, delay);
}

/** The job's end: its options with Use, an ambiguous reply's readings, or a message. */
function showAssistant(unit, token, answer) {
  const box = assistBox(unit);
  if (!box || token !== unit.assist) {
    return;
  }
  if (!record(answer) || !text(answer.message)) {
    box.replaceChildren(element('p', { class: 'assist-message' }, 'The assistant is unavailable right now.'));
    return;
  }
  const heading = element('p', { class: 'assist-message' }, answer.message);
  if (answer.state === 'options' && Array.isArray(answer.options)) {
    const list = element('ul', { class: 'assist-options' });
    for (const option of answer.options) {
      if (!record(option) || ![option.current, option.expression, option.value, option.explanation, option.action].every(text) || !PROPOSAL_PATH.test(option.action)) {
        continue;
      }
      const use = element(
        'button',
        { type: 'button', class: 'assist-use', 'aria-label': `Use ${option.expression} = ${option.value}` },
        'Use',
      );
      use.addEventListener('click', () => applyOption(unit, token, option.action, use));
      list.append(
        element(
          'li',
          {},
          element('span', {}, `Now ${option.current}: `),
          element('bdi', {}, option.expression),
          element('span', {}, ` = ${option.value} \u00b7 `),
          element('bdi', { class: 'assist-explanation' }, option.explanation),
          ' ',
          use,
        ),
      );
    }
    box.replaceChildren(heading, list);
    return;
  }
  if (answer.state === 'readings' && Array.isArray(answer.readings)) {
    const list = element('ul', { class: 'assist-readings' });
    for (const reading of answer.readings.filter(text)) {
      list.append(element('li', {}, element('bdi', {}, reading)));
    }
    box.replaceChildren(heading, list);
    return;
  }
  box.replaceChildren(heading);
}

/** Use: posts the apply route; the answer is a unit save's answer. It is the unit's flight
 * like a save: the draft generation and sequence are taken before the request, a save asked
 * for meanwhile waits in the unit's queue, and page posts wait for it, so a draft typed while
 * Use is on its way is kept. */
async function applyOption(unit, token, action, button) {
  if (token !== unit.assist || unit.flight || unit.orphan) {
    return;
  }
  button.disabled = true;
  unit.seq += 1;
  const flight = { key: null, gen: unit.gen, seq: unit.seq, submitter: null };
  unit.flight = flight;
  pending += 1;
  unit.form.setAttribute('aria-busy', 'true');
  const result = await postForm(action, [['csrf', csrfOf(unit)]]);
  land(unit, () => {
    if (result.status === 200 && valid(result.answer)) {
      stopAssist(unit);
      return settle(unit, flight, result.answer, 200, null);
    }
    if (token === unit.assist) {
      assistText(
        unit,
        record(result.answer) && text(result.answer.message)
          ? result.answer.message
          : 'Not used: the server did not answer; try again.',
      );
    }
    return false;
  });
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
    orphan: false,
    assist: 0,
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

/** Says why a deferred button press was not sent. */
function dropped(intent) {
  if (intent.label === 'Undo') {
    announce('Not undone: this field\u2019s save answered while the Undo waited, so this Undo no longer applies. Check the field and use its Undo again if it still offers one.');
  } else if (intent.label === 'Yes') {
    announce('Not switched: this field\u2019s save answered while the suggestion waited, so it no longer applies. Check the field first.');
  } else {
    announce(`Not sent: the page changed while your entry was saved. Press ${intent.label} again if you still want it.`);
  }
}

/** Sends a page post deferred while saves were pending. Its button may have been rebuilt
 * by a save's answer: the press applies to the rebuilt button only if it is the same
 * action on the same note (never another note's Undo); otherwise nothing is sent. */
function resume(form, submitter, intent) {
  let target = submitter;
  if (intent && !(submitter.isConnected && submitter.form === form && submitter.value === intent.value)) {
    const rebuilt = intent.id ? document.getElementById(intent.id) : null;
    target =
      rebuilt instanceof HTMLButtonElement &&
      rebuilt.isConnected &&
      rebuilt.form &&
      rebuilt.name === intent.name &&
      rebuilt.value === intent.value
        ? rebuilt
        : null;
    if (!target) {
      dropped(intent);
      return;
    }
  }
  const owner = target ? target.form : form;
  if (!owner || !owner.isConnected) {
    dropped(intent || { label: 'the button' });
    return;
  }
  // The submit event, when it fires, fires inside requestSubmit; a form that fails its
  // constraints fires none (the browser shows why), so the mark never outlives the call.
  released.add(owner);
  try {
    owner.requestSubmit(target);
  } catch {
    dropped(intent || { label: 'the button' });
  } finally {
    released.delete(owner);
  }
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
  const submitter =
    event.submitter instanceof HTMLButtonElement && event.submitter.form === form
      ? event.submitter
      : null;
  const intent = submitter && {
    id: submitter.id,
    name: submitter.name,
    value: submitter.value,
    label: submitter.textContent.trim(),
  };
  whenIdle()
    .then(() => {
      waiting.delete(form);
      resume(form, submitter, intent);
    })
    .catch(() => {
      waiting.delete(form);
    });
});

for (const form of document.forms) {
  enhance(form);
}
document.documentElement.setAttribute('data-sheet-assist', 'ready');

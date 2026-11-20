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
/** The only href a catalogue entry link may carry (step 3a): the sheet's own entry route. */
const ENTRY_PATH = /^\/private-sheets\/\d+\/catalogue\/[a-z][a-z0-9-]{0,63}\/[a-z][a-z0-9-]{0,63}$/;
/** The only hrefs the suggestion and name-match code may use (step 4): the sheet's own routes. */
const SUGGEST_PATH = /^\/private-sheets\/\d+\/suggest\/\d+\/\d+$/;
const WHICH_PATH = /^\/private-sheets\/\d+\/cells\/\d+\/\d+\/\d+\/which\?base=\d+$/;
const BROWSE_PATH = /^\/private-sheets\/\d+\/catalogue\/[a-z][a-z0-9-]{0,63}(?:\?type=[a-z][a-z0-9-]{0,63})?$/;
const SLUG = /^[a-z][a-z0-9-]{0,63}$/;
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
/** The reminders (step 3c-1): `false` once their route answered 404 (off, unavailable or not
 * ready). A transient refusal (503: the breaker is not closed) leaves them on. */
let remindersOn = true;

/** The only attribute names `element` sets: no inline styling, no event handler, no URL that
 * loads anything. */
const ATTRIBUTES = new Set([
  'action', 'aria-activedescendant', 'aria-autocomplete', 'aria-controls', 'aria-describedby',
  'aria-expanded', 'aria-label', 'aria-labelledby', 'aria-selected', 'autocomplete', 'class',
  'for', 'form', 'hidden', 'href', 'id', 'method', 'name', 'rel', 'role', 'target', 'type',
  'value',
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

/** " (with N other fields)": the fields a bundle's Undo reverts together with this one. */
function withOthers(actions) {
  const others = typeof actions.with === 'string' && DIGITS.test(actions.with) ? actions.with : '0';
  return others === '0' ? '' : ` (with ${others} other field${others === '1' ? '' : 's'})`;
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
        element('span', { id: `was-${key}` }, `Last change: was ${actions.undo}${withOthers(actions)}`),
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
  showLink(unit, shown.link);
  const matchIds = showMatch(unit, shown.match);
  const noteIds = showNote(unit, shown.note, button);
  const after = unit.form.querySelector('.entry-note') || button;
  const offerIds = showActions(unit, shown.actions, revision, after);
  describe(unit, [...noteIds, ...offerIds, ...matchIds]);
}

/** The catalogue entry link of a saved name cell (step 3a): put back as the page draws it, or
 * dropped when the save left the row without one. An answer without the member leaves it. */
function showLink(unit, link) {
  if (link === undefined) {
    return;
  }
  const old = unit.form.querySelector('.entry-link');
  const cell = /^row-(\d+-\d+-\d+)$/.exec(unit.control.id);
  if (old) {
    old.remove();
  }
  if (link === null || !cell) {
    return;
  }
  const id = `entry-${cell[1]}`;
  unit.control.after(
    element(
      'a',
      {
        class: 'entry-link',
        id,
        href: link.href,
        target: '_blank',
        rel: 'noopener',
        'aria-labelledby': `${id} ${unit.control.id} catalogue-${link.catalogue}`,
      },
      'Open entry',
    ),
  );
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
  if (status >= 200 && status < 300) {
    return 'Not saved as far as this page knows: the server answered but its reply could not be read. Reload the page to see the saved value.';
  }
  const code = status ? ` (HTTP ${status})` : '';
  return `Not saved${code}. Your entry is kept in its field; press Enter or Save to try again, or reload the page.`;
}

const text = (value) => typeof value === 'string';
const textOrNull = (value) => value === null || text(value);
const record = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const STATUSES = new Set(['saved', 'unchanged', 'conflict', 'invalid']);

/** The name match of a saved name cell (step 4): absent or null, or one of the two shapes. */
function validMatch(match) {
  return (
    match === undefined ||
    match === null ||
    (record(match) &&
      ((match.kind === 'several' && text(match.which) && WHICH_PATH.test(match.which)) ||
        (match.kind === 'none' &&
          text(match.browse) &&
          BROWSE_PATH.test(match.browse) &&
          (match.current === undefined || typeof match.current === 'boolean'))))
  );
}

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
    (unit.reminder === undefined || (record(unit.reminder) && text(unit.reminder.key))) &&
    (unit.link === undefined ||
      unit.link === null ||
      (record(unit.link) && ENTRY_PATH.test(unit.link.href) && DIGITS.test(unit.link.catalogue))) &&
    validMatch(unit.match) &&
    (note === null ||
      (record(note) && text(note.kind) && text(note.shown) && text(note.value))) &&
    (actions === null ||
      (record(actions) &&
        text(actions.note) &&
        textOrNull(actions.undo) &&
        (actions.with === undefined || (text(actions.with) && DIGITS.test(actions.with))) &&
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

/** The answer has the shape the module draws from; anything else is a failure. `many`: an
 * apply's answer, which names every changed unit in `units` (step 3b), not one in `unit`. */
function valid(answer, many = false) {
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
    (many
      ? Array.isArray(answer.units) && answer.units.length > 0 && answer.units.every((one) => validUnit(one, refused))
      : validUnit(answer.unit, refused)) &&
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

/** Drops the name-match warning and its inline "Which one?" list, as the server's draft
 * re-render shows none (the control then names no `match-` id in `aria-describedby`). */
function clearMatch(unit) {
  unit.form.querySelector('.entry-match')?.remove();
  unit.form.querySelector('.which-list')?.remove();
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
  clearMatch(unit);
  unit.form.querySelector('.entry-note')?.remove();
  describe(unit, []);
  setAlert(unit, message);
}

/** Applies an answer; returns whether a save queued meanwhile is dropped: after a
 * conflict or an orphaned unit (the player must see that first), not after a refused
 * value (the queued entry is a later one and goes). */
function settle(unit, flight, answer, status, submitter, quiet = false) {
  if (flight.seq < unit.applied) {
    return false;
  }
  unit.applied = flight.seq;
  const pressed = flight.submitter || submitter;
  const focusBack = pressed && document.activeElement === pressed;
  if (!valid(answer)) {
    // A pick refused as malformed (400) would be refused again: it is dropped.
    if (status === 400) {
      dropEntry(unit, flight);
    }
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
    if (answer.status === 'saved') {
      // The decreased value is saved: an older reminder no longer applies; a decrease starts one.
      stopRemind(unit);
      remind(unit, answer.unit.reminder);
    }
    // A pick among duplicates is held until this save's 200 answer (step 4).
    dropEntry(unit, flight);
    unit.sent = answerKey(unit, answer.unit);
    unit.ack = current ? entryKey(unit.form) : unit.sent;
    if (!quiet) {
      announce(answer.message);
    }
    if (focusBack) {
      unit.control.focus();
    }
  } else {
    // The draft stays in its control. After a conflict only Enter or Save sends it again,
    // so it is overwritten consciously, never by leaving the field (ASSIST-23); after a
    // refused value a corrected entry saves as usual.
    unit.hold = answer.status === 'conflict';
    // A refused save (409, 422) drops the pick it carried: the catalogue or the row moved.
    dropEntry(unit, flight);
    // As on the re-rendered page, a drafted unit shows no note, switch or Undo.
    unit.form.querySelector('.entry-note')?.remove();
    clearActions(unit);
    clearMatch(unit);
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
  const picked = unit.form.querySelector('input[name="entry"]');
  const flight = { key, gen: unit.gen, seq: unit.seq, submitter, entry: picked ? picked.value : null };
  unit.flight = flight;
  unit.sent = key;
  pending += 1;
  unit.form.setAttribute('aria-busy', 'true');
  const { status, answer } = await bounded(unit.form.action, body);
  land(unit, () => settle(unit, flight, answer, status, submitter));
}

/** A POST asking for JSON, bounded: one that hangs, before its headers or inside its body,
 * counts as a network failure after 30 s, so a unit's flight, and page posts waiting for it,
 * never wait forever. An unreadable JSON body keeps its HTTP status (a 409 still holds the
 * field); only an abort counts as a network failure (status 0). Never throws. */
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
    let answer = null;
    if (type === JSON_TYPE) {
      // An unreadable body keeps its status: a 409 must still hold the field.
      try {
        answer = await response.json();
      } catch (error) {
        if (error.name === 'AbortError') {
          throw error;
        }
      }
    }
    return { status: response.status, answer };
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
  return boxFor(unit, 'assist');
}

/** The reminder's box, below the decreased field (step 3c-1). */
function remindBox(unit) {
  return boxFor(unit, 'remind');
}

/** The unit's `role="status"` box of a kind (`assist`: an interpretation, `remind`: a
 * reminder), made on first use. */
function boxFor(unit, kind) {
  const shown = names(unit);
  if (!shown) {
    return null;
  }
  let box = document.getElementById(`${kind}-${shown.key}`);
  if (!box) {
    box = element('div', { class: `entry-${kind}`, id: `${kind}-${shown.key}`, role: 'status' });
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

/** Ends the unit's reminder on this page: its polling stops and its box goes. */
function stopRemind(unit) {
  unit.remind += 1;
  const shown = names(unit);
  if (shown) {
    document.getElementById(`remind-${shown.key}`)?.remove();
  }
}

/** The token a job of this kind holds: a newer start or a stop changes it. */
function current(unit, reminder) {
  return reminder ? unit.remind : unit.assist;
}

/** Starts a reminder for a saved decrease and polls it (step 3c-1). The module stays quiet
 * on every refusal and failure: a reminder is never blocking. */
async function remind(unit, reminder) {
  if (!remindersOn || !record(reminder) || !text(reminder.key) || !names(unit)) {
    return;
  }
  const base = sheetPath(unit);
  if (!base) {
    return;
  }
  const token = unit.remind;
  const started = await postForm(`${base}/reminders`, [
    ['csrf', csrfOf(unit)],
    ['note', reminder.key],
  ]);
  if (token !== unit.remind) {
    return;
  }
  if (started.status === 404) {
    remindersOn = false;
    return;
  }
  const answer = started.answer;
  if (started.status === 202 && record(answer) && text(answer.poll) && POLL_PATH.test(answer.poll)) {
    poll(unit, token, answer.poll, 500, true);
  }
}

/** The reminder's end: its hint, the cited row's name and the option with Use, and Dismiss.
 * Anything that is not a well-formed reminder shows nothing. */
function showReminder(unit, token, answer) {
  if (token !== unit.remind) {
    return;
  }
  const options = record(answer) && Array.isArray(answer.options) ? answer.options.filter(validOption) : [];
  if (
    !record(answer) ||
    answer.state !== 'reminder' ||
    !text(answer.hint) ||
    !(answer.row === null || text(answer.row)) ||
    !Array.isArray(answer.options) ||
    options.length !== answer.options.length
  ) {
    document.getElementById(`remind-${names(unit)?.key}`)?.remove();
    return;
  }
  const box = remindBox(unit);
  if (!box) {
    return;
  }
  const lines = [element('p', { class: 'remind-message' }, `Reminder: ${answer.hint}`)];
  if (answer.row !== null) {
    lines.push(element('p', { class: 'remind-row' }, 'About: ', element('bdi', {}, answer.row)));
  }
  const list = element('ul', { class: 'assist-options' });
  for (const option of options) {
    list.append(optionItem(unit, token, option, true));
  }
  const dismiss = element('button', { type: 'button', class: 'remind-dismiss' }, 'Dismiss');
  dismiss.addEventListener('click', () => {
    stopRemind(unit);
  });
  box.replaceChildren(...lines, ...(options.length > 0 ? [list] : []), dismiss);
}

const nonEmptyText = (value) => text(value) && value.length > 0;

/** An option as an interpretation or a reminder offers it: one to four changes, each with its
 * target, the value now and the value it would write, and the entry text behind its literals
 * (an array, empty for an interpretation). An option whose shape is not exactly that is not
 * shown. */
function validOption(option) {
  return (
    record(option) &&
    [option.explanation, option.action].every(text) &&
    PROPOSAL_PATH.test(option.action) &&
    Array.isArray(option.changes) &&
    option.changes.length >= 1 &&
    option.changes.length <= 4 &&
    option.changes.every(
      (change) => record(change) && [change.target, change.before, change.expression, change.after].every(text),
    ) &&
    Array.isArray(option.sources) &&
    option.sources.every(
      (source) => record(source) && [source.entry, source.badge, source.excerpt].every(nonEmptyText),
    )
  );
}

/** The badges that need no warning: the entry text is the catalogue owner's or was reviewed. */
const PLAIN_BADGES = new Set(['Owner-written', 'Reviewed extraction']);

/** One source line: the entry a number of the option comes from and the words around it,
 * with the badge unless the text is owner-written or reviewed. Text nodes only. */
function sourceLine(source) {
  const badge = PLAIN_BADGES.has(source.badge) ? '' : ` (${source.badge.toLowerCase()})`;
  return element(
    'span',
    { class: 'assist-source' },
    'from ',
    element('bdi', {}, source.entry),
    `${badge}: `,
    element('bdi', {}, source.excerpt),
  );
}

/** One option's list item with its Use button. */
function optionItem(unit, token, option, reminder) {
  const summary = option.changes.map((change) => `${change.target} ${change.expression} = ${change.after}`).join('; ');
  const use = element('button', { type: 'button', class: 'assist-use', 'aria-label': `Use ${summary}` }, 'Use');
  use.addEventListener('click', () => applyOption(unit, token, option.action, use, reminder));
  const lines = option.changes.map((change) =>
    element(
      'span',
      { class: 'assist-change' },
      element('bdi', {}, change.target),
      `: now ${change.before}, `,
      element('bdi', {}, change.expression),
      ` = ${change.after}`,
    ),
  );
  const item = element('li', {});
  lines.forEach((line, n) => item.append(n > 0 ? '; ' : '', line));
  item.append(' \u00b7 ', element('bdi', { class: 'assist-explanation' }, option.explanation), ' ', use);
  option.sources.forEach((source) => item.append(element('br', {}), sourceLine(source)));
  return item;
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

function poll(unit, token, path, delay, reminder = false) {
  setTimeout(async () => {
    if (token !== current(unit, reminder) || !document.contains(unit.form)) {
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
    if (token !== current(unit, reminder)) {
      return;
    }
    if (status === 200 && record(answer) && answer.state === 'thinking') {
      poll(unit, token, path, Math.min(2000, Math.round(delay * 1.5)), reminder);
      return;
    }
    if (reminder) {
      showReminder(unit, token, answer);
    } else {
      showAssistant(unit, token, answer);
    }
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
      if (validOption(option)) {
        list.append(optionItem(unit, token, option, false));
      }
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

/** The unit of the page, other than `except`, whose form posts to `action`. */
function unitAt(action, except) {
  for (const form of document.forms) {
    const other = units.get(form);
    if (other && other !== except && new URL(form.action, document.baseURI).pathname === action) {
      return other;
    }
  }
  return null;
}

/** An apply's answer names every changed unit. The unit that asked draws its own member as a
 * save answer; when it is not among them (a reminder's Use from a field it does not change)
 * its flight just ends, announcing the message, and it is not drawn. Every other member's
 * unit is redrawn like a save answer too, but only when it is idle: a unit with a request in
 * flight (its own answer is the later one), an orphan, a held unit (ASSIST-23: only Enter or
 * Save resend) or one holding a draft keeps its base and its control, so its next save meets
 * the normal conflict instead of overwriting the applied value. The message is announced
 * once. */
function settleApplied(unit, flight, answer) {
  const own = new URL(unit.form.action, document.baseURI).pathname;
  const mine = answer.units.find((one) => one.action === own);
  let drop = false;
  if (mine) {
    drop = settle(unit, flight, { ...answer, unit: mine }, 200, null);
  } else {
    unit.applied = Math.max(unit.applied, flight.seq);
    announce(answer.message);
  }
  for (const member of answer.units) {
    if (member === mine || typeof member.action !== 'string') {
      continue;
    }
    const other = unitAt(member.action, unit);
    if (!other || other.flight || other.orphan || other.hold) {
      continue;
    }
    if (entryKey(other.form) !== other.ack) {
      continue;
    }
    other.seq += 1;
    settle(other, { key: null, gen: other.gen, seq: other.seq, submitter: null }, { ...answer, unit: member }, 200, null, true);
  }
  if (!mine) {
    project(answer, BigInt(answer.revision));
  }
  return drop;
}

/** Use: posts the apply route; the answer names every changed unit in `units`. It is the unit's flight
 * like a save: the draft generation and sequence are taken before the request, a save asked
 * for meanwhile waits in the unit's queue, and page posts wait for it, so a draft typed while
 * Use is on its way is kept. */
async function applyOption(unit, token, action, button, reminder = false) {
  if (token !== current(unit, reminder) || unit.flight || unit.orphan) {
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
    if (result.status === 200 && valid(result.answer, true)) {
      if (reminder) {
        stopRemind(unit);
      } else {
        stopAssist(unit);
      }
      return settleApplied(unit, flight, result.answer);
    }
    if (token === current(unit, reminder)) {
      const message =
        record(result.answer) && text(result.answer.message)
          ? result.answer.message
          : 'Not used: the server did not answer; try again.';
      if (reminder) {
        const box = remindBox(unit);
        box?.replaceChildren(element('p', { class: 'remind-message' }, message));
      } else {
        assistText(unit, message);
      }
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
    remind: 0,
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
    // A name that is Not set takes no entry (the server refuses the pair).
    if (event.target instanceof HTMLSelectElement && event.target.value === 'absent') {
      form.querySelector('input[name="entry"]')?.remove();
    }
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
  form.addEventListener('click', (event) => {
    const link = event.target instanceof Element ? event.target.closest('a.which') : null;
    if (link && form.contains(link)) {
      event.preventDefault();
      whichList(unit, link);
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

// Name suggestions and the name match (step 4; docs/assisted-entry.md, "Catalogue links").
// A catalogue-backed name input (`data-suggest`, the sheet's own suggest route) becomes an
// ARIA combobox; a saved name cell under rule 3 or 4 shows its warning, "Which one?" inline
// and "Did you mean...?". Everything the server names is drawn with textContent inside <bdi>;
// hrefs are checked against fixed patterns first. Without this module the page keeps its
// warnings, Browse and the "Which one?" page (D-G).

const SUGGEST_DEBOUNCE = 150;
const SUGGEST_TIMEOUT = 5000;
const MAX_SUGGESTIONS = 8;

/** Removes the pick among duplicates (`entry`) the save that carried it has answered for. */
function dropEntry(unit, flight) {
  const held = unit.form.querySelector('input[name="entry"]');
  if (held && flight && flight.entry === held.value) {
    held.remove();
  }
}

/** Holds a pick among duplicates in its form until the next save (never rendered empty). */
function holdEntry(form, key) {
  form.querySelector('input[name="entry"]')?.remove();
  form.append(element('input', { type: 'hidden', name: 'entry', value: key }));
}

function addTypo(unit, line) {
  if (!unit.suggest || line.querySelector('.did-you-mean')) {
    return;
  }
  const button = element('button', { type: 'button', class: 'did-you-mean' }, 'Did you mean…?');
  button.addEventListener('click', () => unit.suggest(true));
  line.append(' ', button);
}

/** The warning of a saved name cell with several or no catalogue match (rules 3 and 4):
 * put back as the page draws it, or dropped when the save left the row linked. An answer
 * without the member leaves it. Returns the ids that describe the control. */
function showMatch(unit, match) {
  const old = unit.form.querySelector('.entry-match');
  if (match === undefined) {
    return old ? [old.id] : [];
  }
  old?.remove();
  unit.form.querySelector('.which-list')?.remove();
  const shown = names(unit);
  if (match === null || !shown) {
    return [];
  }
  const line = element('p', { class: 'entry-match', id: `match-${shown.key}` });
  if (match.kind === 'several') {
    line.append(
      'Several catalogue entries have this name. ',
      element('a', { class: 'which', href: match.which }, 'Which one?'),
    );
  } else {
    line.append(
      match.current === false
        ? 'No current entry of this type has this name. '
        : 'Not in the catalogue. ',
      element('a', { href: match.browse, target: '_blank', rel: 'noopener' }, 'Browse'),
    );
    addTypo(unit, line);
  }
  (unit.form.querySelector('.entry-link') || unit.control).after(line);
  return [line.id];
}

/** The candidates of "Which one?" inline, under the warning; any failure sends the player to
 * the page the link names. */
async function whichList(unit, link) {
  const href = link.getAttribute('href');
  if (!href || !WHICH_PATH.test(href)) {
    return;
  }
  unit.form.querySelector('.which-list')?.remove();
  // Clicking the link left the field, which saved it: the route is under the runtime permit,
  // so it is asked only once every save in flight has its answer.
  await whenIdle();
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), SUGGEST_TIMEOUT);
  let answer = null;
  let status = 0;
  try {
    const response = await fetch(href, {
      signal: controller.signal,
      headers: { Accept: JSON_TYPE },
      credentials: 'same-origin',
      cache: 'no-store',
      redirect: 'manual',
    });
    status = response.status;
    const type = (response.headers.get('Content-Type') || '').split(';')[0].trim();
    if (response.status === 200 && type === JSON_TYPE) {
      answer = await response.json();
    }
  } catch {
    answer = null;
  } finally {
    clearTimeout(timer);
  }
  if (status === 429 || status === 503) {
    // Busy: say so and let the player press the link again; do not leave the sheet.
    setAlert(unit, 'The catalogue is busy. Press Which one? again in a moment.');
    return;
  }
  const valid =
    record(answer) &&
    answer.status === 'ok' &&
    typeof answer.more === 'boolean' &&
    textOrNull(answer.browse) &&
    (answer.browse === null || BROWSE_PATH.test(answer.browse)) &&
    Array.isArray(answer.candidates) &&
    answer.candidates.length > 0 &&
    answer.candidates.every(
      (one) =>
        record(one) &&
        text(one.key) &&
        SLUG.test(one.key) &&
        text(one.type) &&
        text(one.hint) &&
        text(one.open) &&
        ENTRY_PATH.test(one.open),
    );
  if (!valid) {
    window.location.assign(href);
    return;
  }
  // A busy answer's alert does not outlive the retry that opened the list.
  clearAlert(unit);
  const list = element('ul', { class: 'which-list' });
  for (const [at, one] of answer.candidates.entries()) {
    const use = element(
      'button',
      { type: 'button', id: `${unit.control.id}-use-${at}`, 'aria-labelledby': `${unit.control.id}-use-${at} ${unit.control.id}-line-${at}` },
      'Use this one',
    );
    use.addEventListener('click', () => {
      holdEntry(unit.form, one.key);
      unit.gen += 1;
      list.remove();
      request(unit, true, null);
    });
    list.append(
      element(
        'li',
        {},
        element(
          'bdi',
          { id: `${unit.control.id}-line-${at}` },
          one.hint ? `${one.type} · ${one.key} · ${one.hint}` : `${one.type} · ${one.key}`,
        ),
        ' ',
        element('a', { href: one.open, target: '_blank', rel: 'noopener' }, 'Open entry'),
        ' ',
        use,
      ),
    );
  }
  if (answer.more && answer.browse) {
    list.append(
      element('li', {}, element('a', { href: answer.browse, target: '_blank', rel: 'noopener' }, 'Browse')),
    );
  }
  unit.form.querySelector('.entry-match')?.after(list);
}

const validSuggestions = (answer) =>
  record(answer) &&
  answer.status === 'ok' &&
  typeof answer.more === 'boolean' &&
  typeof answer.cut === 'boolean' &&
  Array.isArray(answer.suggestions) &&
  answer.suggestions.length <= MAX_SUGGESTIONS &&
  answer.suggestions.every(
    (one) =>
      record(one) &&
      text(one.name) &&
      text(one.key) &&
      SLUG.test(one.key) &&
      text(one.type) &&
      typeof one.duplicate === 'boolean' &&
      text(one.hint),
  );

/** The combobox of one name input. `unit` is its saved-cell unit, or null for the new-row
 * form. State: a generation counter and one AbortController, so an answer is drawn only for
 * the current generation and the current text. */
function combobox(control, path, unit) {
  const id = `suggest-${control.id.replace(/^row-/, '')}`;
  const list = element('ul', { role: 'listbox', id, 'aria-label': 'Catalogue suggestions', hidden: '' });
  const status = element('p', { role: 'status', class: 'visually-hidden', id: `${id}-status` });
  control.after(list, status);
  control.setAttribute('role', 'combobox');
  control.setAttribute('aria-autocomplete', 'list');
  control.setAttribute('aria-controls', id);
  control.setAttribute('aria-expanded', 'false');
  control.setAttribute('autocomplete', 'off');
  const box = { items: [], active: -1, gen: 0, abort: null, timer: 0, composing: false };

  const activate = (index) => {
    box.active = index;
    [...list.children].forEach((option, at) => {
      option.setAttribute('aria-selected', at === index ? 'true' : 'false');
    });
    if (index >= 0 && list.children[index]) {
      control.setAttribute('aria-activedescendant', list.children[index].id);
    } else {
      control.removeAttribute('aria-activedescendant');
    }
  };

  const close = () => {
    box.gen += 1;
    box.abort?.abort();
    clearTimeout(box.timer);
    box.items = [];
    list.replaceChildren();
    list.hidden = true;
    control.setAttribute('aria-expanded', 'false');
    activate(-1);
  };

  const draw = (answer, typo) => {
    list.replaceChildren();
    box.items = answer.suggestions.map((one) => ({ ...one }));
    answer.suggestions.forEach((one, at) => {
      const attributes = { role: 'option', id: `${id}-${at}`, 'aria-selected': 'false' };
      if (one.duplicate) {
        attributes['aria-label'] = `${one.name}, ${one.type}, ${one.hint || one.key}`;
      }
      const option = element('li', attributes, element('bdi', {}, one.name), ` · ${one.type}`);
      if (one.duplicate) {
        option.append(' · ', element('bdi', {}, one.hint || one.key));
      }
      list.append(option);
    });
    if (answer.suggestions.length === 0 && !typo) {
      box.items.push({ typo: true });
      list.append(
        element('li', { role: 'option', id: `${id}-typo`, 'aria-selected': 'false' }, 'Did you mean…?'),
      );
    }
    const count = answer.suggestions.length;
    status.textContent = count === 0 ? 'No suggestions' : `${count} suggestion${count === 1 ? '' : 's'}`;
    list.hidden = box.items.length === 0;
    control.setAttribute('aria-expanded', list.hidden ? 'false' : 'true');
    activate(-1);
  };

  const query = async (typo) => {
    clearTimeout(box.timer);
    const typed = control.value.trim();
    const scalars = Array.from(typed).length;
    if (scalars < (typo ? 3 : 2) || scalars > 64) {
      close();
      return;
    }
    box.gen += 1;
    const gen = box.gen;
    box.abort?.abort();
    const controller = new AbortController();
    box.abort = controller;
    const timer = setTimeout(() => controller.abort(), SUGGEST_TIMEOUT);
    const params = new URLSearchParams({ q: typed });
    if (typo) {
      params.set('typo', '1');
    }
    let answer = null;
    try {
      const response = await fetch(`${path}?${params}`, {
        signal: controller.signal,
        headers: { Accept: JSON_TYPE },
        credentials: 'same-origin',
        cache: 'no-store',
        redirect: 'manual',
      });
      const type = (response.headers.get('Content-Type') || '').split(';')[0].trim();
      if (response.status === 200 && type === JSON_TYPE) {
        answer = await response.json();
      }
    } catch {
      answer = null;
    } finally {
      clearTimeout(timer);
    }
    if (gen !== box.gen || control.value.trim() !== typed) {
      return;
    }
    if (!validSuggestions(answer)) {
      // 429, 503 and any failure close the list silently.
      close();
      return;
    }
    draw(answer, typo);
  };

  const pick = (index) => {
    const item = box.items[index];
    if (!item) {
      return;
    }
    if (item.typo) {
      close();
      control.focus();
      query(true);
      return;
    }
    control.value = item.name;
    control.form?.querySelector('input[name="entry"]')?.remove();
    if (item.duplicate && control.form) {
      holdEntry(control.form, item.key);
    }
    if (unit) {
      unit.gen += 1;
    }
    close();
    status.textContent = `${item.name} chosen`;
  };

  if (unit) {
    unit.suggest = (typo) => {
      control.focus();
      query(typo);
    };
  }
  control.addEventListener('input', (event) => {
    // Typing drops a held pick; no query while composing.
    control.form?.querySelector('input[name="entry"]')?.remove();
    if (event.isComposing || box.composing) {
      return;
    }
    clearTimeout(box.timer);
    box.timer = setTimeout(() => query(false), SUGGEST_DEBOUNCE);
  });
  if (!unit && control.form) {
    // The new-row form is not a unit: a name set to Not set drops a held pick there too (the
    // server refuses `entry` with an absent name).
    control.form.addEventListener('change', (event) => {
      if (event.target instanceof HTMLSelectElement && event.target.value === 'absent') {
        control.form.querySelector('input[name="entry"]')?.remove();
      }
    });
  }
  control.addEventListener('compositionstart', () => {
    box.composing = true;
    clearTimeout(box.timer);
  });
  control.addEventListener('compositionend', () => {
    box.composing = false;
    query(false);
  });
  control.addEventListener('keydown', (event) => {
    if (event.isComposing || event.keyCode === 229) {
      return;
    }
    const open = !list.hidden;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      if (open) {
        activate((box.active + 1) % box.items.length);
      } else {
        query(false);
      }
    } else if (event.key === 'ArrowUp' && open) {
      event.preventDefault();
      activate(box.active <= 0 ? box.items.length - 1 : box.active - 1);
    } else if (event.key === 'Enter' && open && box.active >= 0) {
      // Selecting fills the name; it does not save.
      event.preventDefault();
      pick(box.active);
    } else if (event.key === 'Escape' && open) {
      event.preventDefault();
      close();
    } else if (event.key === 'Tab') {
      close();
    }
  });
  control.addEventListener('focusout', () => close());
  // Pointer and touch picks keep focus in the input (pointerdown and mousedown are
  // prevented), so the save on leaving never sends the half-typed text; the pick itself runs
  // on `click`, after the press has ended, so the click can never land on what the closing
  // list uncovers.
  list.addEventListener('pointerdown', (event) => event.preventDefault());
  list.addEventListener('mousedown', (event) => event.preventDefault());
  list.addEventListener('click', (event) => {
    event.preventDefault();
    const option = event.target instanceof Element ? event.target.closest('[role="option"]') : null;
    if (option && list.contains(option)) {
      pick([...list.children].indexOf(option));
    }
  });
}

for (const control of document.querySelectorAll('input[data-suggest]')) {
  const path = control.getAttribute('data-suggest');
  if (control instanceof HTMLInputElement && path && SUGGEST_PATH.test(path)) {
    const unit = control.form ? units.get(control.form) : undefined;
    combobox(control, path, unit || null);
    const line = unit && unit.form.querySelector('.entry-match');
    if (line && !line.querySelector('a.which')) {
      addTypo(unit, line);
    }
  }
}
document.documentElement.setAttribute('data-sheet-assist', 'ready');

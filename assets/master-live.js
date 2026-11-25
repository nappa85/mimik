// Mimik Master workspace module (CAMP-14, ROAD-8; docs/campaigns.md, "Event stream", and
// docs/assisted-entry.md, "Master workspace script"). Progressive enhancement of the plain
// workspace page: the browser's native EventSource (no htmx: DEP-33) follows
// `GET /campaigns/{id}/master/events` and the lists update in place from its `snapshot`
// events. Without this module the server-rendered page, its Reload link, the paging links
// and the Open links keep working.
//
// The page runs under the strict CSP with Trusted Types and no policy: this module builds DOM
// with createElement and textContent only, sets no inline styling, evaluates nothing and has no
// SSE parser of its own. Rows keep their elements (updated in place, matched by sheet ID), so
// focus, selection and scroll stay put; a focused Open link is never replaced.
//
// Connection contract: a `status` event `live` carries the tab's stream token; `replaced` is
// terminal (a newer tab of this Master took the slot: only an explicit Resume reconnects);
// `closing` and `error` end the stream. After any stream failure the module closes the
// EventSource and probes the finite snapshot (`.../master/snapshot`, with `?stream=<token>`
// once it has one), which tells what EventSource cannot (it exposes no HTTP status): 401 shows
// a sign-in link, 403 clears the lists, 429 and 503 back off, a `replaced` stream state does
// not reconnect, anything else retries after a jittered 3 to 30 s.

const DIGITS = /^\d+$/;
const TOKEN = /^[0-9a-f]{32}$/;
const RETRY_MIN = 3000;
const RETRY_MAX = 30000;
const SECTIONS = ['players', 'npcs'];
const NEXT_LABEL = { players: 'Next player sheets', npcs: 'Next NPC sheets' };
const EMPTY_LABEL = { players: 'No player sheets', npcs: 'No NPC sheets' };

const root = document.getElementById('master-live');
const campaign = root?.dataset.campaign;
const after = { players: root?.dataset.playersAfter, npcs: root?.dataset.npcsAfter };

/** The one live connection, its tab token, and the retry timer: at most one of each. */
let source = null;
let token = null;
let timer = null;
let probing = false;
/** Incremented whenever pending asynchronous work must be discarded. */
let generation = 0;
/** `true` after a terminal state (replaced, signed out, access ended): no automatic retry. */
let stopped = false;
let attempt = 0;
let statusKey = '';

function textIn(parent, selector, text) {
  const element = parent.querySelector(selector);
  if (element.textContent !== text) element.textContent = text;
}

function paged(key) {
  return Number(after[key]) > 0;
}

function pageQuery(key, value) {
  const players = key === 'players' ? value : after.players;
  const npcs = key === 'npcs' ? value : after.npcs;
  return `players_after=${players}&npcs_after=${npcs}`;
}

function badge(row) {
  if (row.seen === null) return 'Not opened';
  return BigInt(row.seen) < BigInt(row.revision) ? 'Changed' : 'Opened';
}

function openHref(row) {
  return `/private-sheets/${row.id}?players_after=${after.players}&npcs_after=${after.npcs}`;
}

/** One row element, the same markup the server renders. */
function build(row) {
  const article = document.createElement('article');
  article.dataset.id = row.id;
  const open = document.createElement('a');
  open.className = 'open';
  open.append(document.createElement('bdi'));
  const custodian = document.createElement('bdi');
  custodian.className = 'custodian';
  const state = document.createElement('span');
  state.className = 'state';
  const revision = document.createElement('span');
  revision.className = 'revision';
  const mark = document.createElement('strong');
  mark.className = 'badge';
  article.append(
    open, ' · custodian ', custodian, ' · ', state, ' · revision ', revision, ' · ', mark,
  );
  return article;
}

/** Brings a row element to a row's values, touching only what changed. */
function update(article, row) {
  const open = article.querySelector('a.open');
  const href = openHref(row);
  if (open.getAttribute('href') !== href) open.setAttribute('href', href);
  textIn(open, 'bdi', row.name);
  textIn(article, '.custodian', row.custodian);
  textIn(article, '.state', row.state);
  textIn(article, '.revision', row.revision);
  textIn(article, '.badge', badge(row));
}

function syncNext(key, rows, next) {
  let anchor = document.getElementById(`${key}-next`);
  if (next === null) {
    anchor?.remove();
    return;
  }
  const href = `/campaigns/${campaign}/master?${pageQuery(key, next)}`;
  if (!anchor) {
    anchor = document.createElement('a');
    anchor.id = `${key}-next`;
    anchor.textContent = NEXT_LABEL[key];
    rows.after(anchor);
  }
  if (anchor.getAttribute('href') !== href) anchor.setAttribute('href', href);
}

/** Replaces one section's rows by `data` in place: by ID, in the server's order. */
function syncSection(key, data) {
  const rows = document.getElementById(`${key}-rows`);
  const existing = new Map();
  for (const article of rows.querySelectorAll(':scope > article')) {
    existing.set(article.dataset.id, article);
  }
  const wanted = new Set(data.rows.map((row) => row.id));
  const active = document.activeElement;
  let focusLost = false;
  for (const [id, article] of existing) {
    if (wanted.has(id)) continue;
    if (article.contains(active)) focusLost = true;
    article.remove();
    existing.delete(id);
  }
  const empty = rows.querySelector(':scope > p.empty');
  if (data.rows.length === 0) {
    const text = `${EMPTY_LABEL[key]}${paged(key) ? ' on this page' : ''}.`;
    if (!empty) {
      const note = document.createElement('p');
      note.className = 'empty';
      note.textContent = text;
      rows.prepend(note);
    } else if (empty.textContent !== text) {
      empty.textContent = text;
    }
  } else {
    empty?.remove();
  }
  // Only new rows move: every existing row is already in the server's order.
  let cursor = rows.querySelector(':scope > article');
  for (const row of data.rows) {
    let article = existing.get(row.id);
    if (!article) article = build(row);
    update(article, row);
    if (article === cursor) {
      cursor = cursor.nextElementSibling;
    } else {
      rows.insertBefore(article, cursor);
    }
  }
  syncNext(key, rows, data.next);
  if (focusLost) document.getElementById(`${key}-heading`).focus({ preventScroll: true });
}

function isString(value) {
  return typeof value === 'string';
}

function validRow(row, npc) {
  return row !== null && typeof row === 'object'
    && isString(row.id) && DIGITS.test(row.id)
    && isString(row.name) && isString(row.custodian) && isString(row.state)
    && isString(row.revision) && DIGITS.test(row.revision)
    && row.npc === npc
    && (row.seen === null || (isString(row.seen) && DIGITS.test(row.seen)));
}

/** The snapshot object, or `null` when it is not exactly the documented shape. */
function parseSnapshot(text) {
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    return null;
  }
  for (const key of SECTIONS) {
    const section = data?.[key];
    if (!section || !Array.isArray(section.rows)) return null;
    if (section.next !== null && !(isString(section.next) && DIGITS.test(section.next))) {
      return null;
    }
    if (!section.rows.every((row) => validRow(row, key === 'npcs'))) return null;
  }
  return data;
}

function apply(data) {
  for (const key of SECTIONS) syncSection(key, data[key]);
}

function clearLists() {
  for (const key of SECTIONS) syncSection(key, { rows: [], next: null });
}

/** The polite connection status. It changes (and is announced) only when the state does. */
function setStatus(key, text, controls = []) {
  document.documentElement.dataset.masterLive = key;
  if (key === statusKey) return;
  statusKey = key;
  document.getElementById('live-status').textContent = text;
  const actions = document.getElementById('live-actions');
  // A used control leaves with its state: focus moves to the Reload link, not to the page.
  const held = actions.contains(document.activeElement);
  actions.replaceChildren(...controls);
  if (held) document.getElementById('reload').focus({ preventScroll: true });
}

function button(label, onClick) {
  const element = document.createElement('button');
  element.type = 'button';
  element.textContent = label;
  element.addEventListener('click', onClick);
  return element;
}

function closeStream() {
  if (source) source.close();
  source = null;
}

/** Stops everything in flight: the stream, the probe and the retry timer. */
function halt() {
  closeStream();
  generation += 1;
  probing = false;
  if (timer !== null) clearTimeout(timer);
  timer = null;
}

function stop(key, text, controls) {
  halt();
  stopped = true;
  setStatus(key, text, controls);
}

function resume() {
  stopped = false;
  attempt = 0;
  token = null;
  statusKey = '';
  connect();
}

function retry(floor = 0) {
  if (stopped || timer !== null) return;
  const high = Math.min(RETRY_MAX, RETRY_MIN * 2 ** (attempt + 1));
  attempt += 1;
  const jittered = RETRY_MIN + Math.random() * (high - RETRY_MIN);
  // The jittered delay is bounded by RETRY_MAX; the server's Retry-After is a floor, never capped.
  const delay = Math.max(jittered, floor);
  timer = setTimeout(() => {
    timer = null;
    connect();
  }, delay);
}

const STALE = 'Reconnecting; the list may be stale.';

/** "Reconnecting" for a retry in progress, but an ongoing outage keeps its own state
 * (offline, busy), so the polite region does not announce the same cycle again and again. */
function reconnecting() {
  if (statusKey === 'offline' || statusKey === 'busy') return;
  setStatus('reconnecting', STALE);
}

/** After any stream failure: close, probe the finite snapshot, then decide. */
async function fail() {
  closeStream();
  if (stopped || timer !== null || probing) return;
  reconnecting();
  probing = true;
  const mine = generation;
  let response;
  try {
    const stream = token === null ? '' : `&stream=${token}`;
    response = await fetch(
      `/campaigns/${campaign}/master/snapshot?${pageQuery()}${stream}`,
      { credentials: 'same-origin', cache: 'no-store', redirect: 'manual' },
    );
  } catch {
    if (mine !== generation) return;
    probing = false;
    setStatus('offline', 'Offline; the list may be stale. Retrying.');
    retry();
    return;
  }
  if (mine !== generation) return;
  const { status } = response;
  if (status === 401) {
    stop('signin', 'Your session ended; the list may be stale.', [
      Object.assign(document.createElement('a'), { href: '/auth/login', textContent: 'Sign in' }),
      button('Try again', resume),
    ]);
  } else if (status === 403) {
    clearLists();
    stop('denied', 'You no longer have access to this workspace; the lists were cleared.');
  } else if (status === 429 || status === 503) {
    probing = false;
    setStatus('busy', 'The server is busy; the list may be stale. Retrying.');
    const seconds = Number(response.headers.get('retry-after'));
    retry(Number.isFinite(seconds) ? seconds * 1000 : 0);
  } else if (status === 200) {
    let data = null;
    try {
      data = parseSnapshot(await response.text());
    } catch {
      data = null;
    }
    if (mine !== generation) return;
    probing = false;
    if (data) apply(data);
    if (response.headers.get('x-master-stream') === 'replaced') {
      replaced();
    } else {
      retry();
    }
  } else {
    probing = false;
    retry();
  }
}

function replaced() {
  stop('replaced', 'Live updates moved to another tab of yours; this list may be stale.', [
    button('Resume live updates', resume),
  ]);
}

function onStatus(event) {
  let message;
  try {
    message = JSON.parse(event.data);
  } catch {
    message = null;
  }
  if (message?.code === 'live' && isString(message.stream) && TOKEN.test(message.stream)) {
    token = message.stream;
    attempt = 0;
    setStatus('live', 'Live: this list updates by itself.');
  } else if (message?.code === 'replaced') {
    closeStream();
    replaced();
  } else {
    fail();
  }
}

function connect() {
  if (source || timer !== null || probing || stopped) return;
  if (statusKey === '') setStatus('connecting', 'Connecting to live updates.');
  else reconnecting();
  token = null;
  const stream = new EventSource(`/campaigns/${campaign}/master/events?${pageQuery()}`);
  source = stream;
  stream.addEventListener('status', (event) => {
    if (source === stream) onStatus(event);
  });
  stream.addEventListener('snapshot', (event) => {
    if (source !== stream) return;
    const data = parseSnapshot(event.data);
    if (data) apply(data);
    else fail();
  });
  stream.addEventListener('error', () => {
    if (source === stream) fail();
  });
}

if (root && DIGITS.test(campaign) && DIGITS.test(after.players) && DIGITS.test(after.npcs)
  && typeof EventSource === 'function') {
  // A page leaving closes its stream; a page shown again with `persisted` (back/forward
  // cache, which the `no-store` pages normally prevent) opens exactly one new one, never two.
  window.addEventListener('pagehide', halt);
  window.addEventListener('pageshow', (event) => {
    if (event.persisted && !stopped) connect();
  });
  connect();
}

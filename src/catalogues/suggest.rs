//! Name suggestions for the sheet's catalogue-backed name cells (assisted sheet step 4, plan
//! item 8 and 9): the in-memory name cache and the matching over it.
//!
//! The cache holds, per sealed catalogue version (keyed by version id and content hash, so a
//! new seal is a new key and old ones age out), the entry type labels and per entry its key,
//! name, stored norm (`catalogue_entry_names`, the table linking on view compares), length in
//! scalars, type and retired flag; no description text. It is one process-wide LRU bounded by
//! bytes; an index larger than the budget serves its request and is not kept.
//!
//! Matching is pure and runs outside the database mutex. `q` is already the folded norm
//! ([`crate::text_fold::name_norm`], the linking rule: no other folding and no
//! language-specific rule). Tiers: the norm starts with `q`; the norm contains `q`; and, only
//! on request, a typo pass (optimal string alignment distance, a transposition costs 1) within
//! a wall-clock budget. Only eligible entries (not retired, of the member's entry type) count
//! at every tier.
use crate::{
    AppError,
    catalogues::read::LABEL_BYTES,
    sources::search::{Budget, QUERY_BUDGET, guarded, interrupted, read_guarded},
};
use hashlink::LruCache;
use rusqlite::Connection;
use std::{
    sync::{Arc, LazyLock, Mutex, PoisonError},
    time::Duration,
};

/// Bytes of the process-wide name cache.
pub(crate) const CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Suggestions in one answer.
pub(crate) const MAX_SUGGESTIONS: usize = 8;
/// The typo pass's wall-clock budget.
pub(crate) const TYPO_BUDGET: Duration = Duration::from_millis(20);
/// Byte guard of a stored norm: NFKC expands one scalar to at most 18.
const NORM_BYTES: usize = 8192;

/// One entry of a version, as the matching needs it.
pub(crate) struct Entry {
    pub key: String,
    pub name: String,
    pub norm: String,
    /// Length of `norm` in scalars.
    pub scalars: usize,
    /// Index into [`NameIndex::types`].
    pub kind: usize,
    pub retired: bool,
}

/// The names of one sealed catalogue version.
pub(crate) struct NameIndex {
    /// `(type id, label)` in the version's order.
    pub types: Vec<(String, String)>,
    pub entries: Vec<Entry>,
}

impl NameIndex {
    /// The bytes charged for the index: string capacities plus fixed overhead.
    pub(crate) fn bytes(&self) -> usize {
        let types: usize = self
            .types
            .iter()
            .map(|(id, label)| {
                id.capacity() + label.capacity() + std::mem::size_of::<(String, String)>()
            })
            .sum();
        let entries: usize = self
            .entries
            .iter()
            .map(|entry| {
                entry.key.capacity()
                    + entry.name.capacity()
                    + entry.norm.capacity()
                    + std::mem::size_of::<Entry>()
            })
            .sum();
        std::mem::size_of::<Self>() + types + entries
    }
}

/// What a cache entry is keyed by: the sealed version and its content hash.
pub(crate) type Key = (i64, String);

struct Inner {
    lru: LruCache<Key, (Arc<NameIndex>, usize)>,
    bytes: usize,
}

/// An LRU of name indexes bounded by bytes.
pub(crate) struct NameCache {
    inner: Mutex<Inner>,
    budget: usize,
}

impl NameCache {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                lru: LruCache::new_unbounded(),
                bytes: 0,
            }),
            budget,
        }
    }

    /// The index of `key`, marked most recently used.
    pub(crate) fn get(&self, key: &Key) -> Option<Arc<NameIndex>> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.lru.get(key).map(|(index, _)| index.clone())
    }

    /// Keeps `index` under `key`, evicting the least recently used until the budget holds; an
    /// index over the budget is not kept.
    pub(crate) fn insert(&self, key: Key, index: Arc<NameIndex>) {
        let size = index.bytes();
        if size > self.budget {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, old)) = inner.lru.insert(key, (index, size)) {
            inner.bytes -= old;
        }
        inner.bytes += size;
        while inner.bytes > self.budget {
            let Some((_, (_, evicted))) = inner.lru.remove_lru() else {
                break;
            };
            inner.bytes -= evicted;
        }
    }

    /// The bytes held now.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bytes
    }

    /// The indexes held now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lru
            .len()
    }
}

/// The process-wide cache.
pub(crate) static NAMES: LazyLock<NameCache> = LazyLock::new(|| NameCache::new(CACHE_BYTES));

/// Reads the names of `version_id` (the latest sealed version's stored norms, so suggestions
/// agree with linking) under the query budget.
///
/// # Errors
/// Storage failures, damaged text, or the time budget.
pub(crate) fn load(db: &Connection, version_id: i64) -> Result<NameIndex, AppError> {
    let _budget = Budget::start(db, QUERY_BUDGET);
    let mut types = Vec::new();
    {
        let mut query = db
            .prepare_cached(&format!(
                "SELECT type_id,{} FROM catalogue_entry_types WHERE version_id=?1 ORDER BY ordinal",
                guarded("label", LABEL_BYTES)
            ))
            .map_err(interrupted)?;
        let rows = query
            .query_map([version_id], |row| {
                Ok((row.get::<_, String>(0)?, read_guarded(row, 1)?))
            })
            .map_err(interrupted)?;
        for row in rows {
            let (id, label) = row.map_err(interrupted)?;
            types.push((id, label));
        }
    }
    let mut entries = Vec::new();
    {
        let mut query = db
            .prepare_cached(&format!(
                "SELECT e.key,{},{},e.type_id,e.retired FROM catalogue_entries e JOIN catalogue_entry_names n ON n.version_id=e.version_id AND n.entry_key=e.key WHERE e.version_id=?1 ORDER BY e.ordinal",
                guarded("e.name", LABEL_BYTES),
                guarded("n.norm", NORM_BYTES)
            ))
            .map_err(interrupted)?;
        let rows = query
            .query_map([version_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    read_guarded(row, 1)?,
                    read_guarded(row, 2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(interrupted)?;
        for row in rows {
            let (key, name, norm, kind, retired) = row.map_err(interrupted)?;
            let kind = types
                .iter()
                .position(|(id, _)| *id == kind)
                .ok_or(AppError::Invalid("Catalogue entry type missing."))?;
            let scalars = norm.chars().count();
            let mut entry = Entry {
                key,
                name,
                norm,
                scalars,
                kind,
                retired: retired != 0,
            };
            entry.key.shrink_to_fit();
            entry.name.shrink_to_fit();
            entry.norm.shrink_to_fit();
            entries.push(entry);
        }
    }
    entries.shrink_to_fit();
    Ok(NameIndex { types, entries })
}

/// A folded query.
pub(crate) struct Query<'a> {
    /// `name_norm` of what the player typed.
    pub norm: &'a str,
    /// Its length in scalars.
    pub scalars: usize,
    /// Whether the typo pass is wanted.
    pub typo: bool,
}

/// One suggestion.
pub(crate) struct Found<'a> {
    pub entry: &'a Entry,
    /// More than one eligible entry shares the norm.
    pub duplicate: bool,
}

/// The answer of [`suggest`].
pub(crate) struct Suggestions<'a> {
    pub items: Vec<Found<'a>>,
    /// More than [`MAX_SUGGESTIONS`] entries matched.
    pub more: bool,
    /// The typo pass reached its budget.
    pub cut: bool,
}

/// The largest edit distance a typo may have: 1 for 3 to 5 scalars, 2 from 6.
fn allowed(scalars: usize) -> usize {
    if scalars >= 6 { 2 } else { 1 }
}

/// Suggestions for `query` among the eligible entries of `index` (not retired, of
/// `entry_type` when given; none when the version does not declare that type). `elapsed`
/// reads the typo pass's clock (a seam for tests); it is checked before every comparison.
pub(crate) fn suggest<'a>(
    index: &'a NameIndex,
    entry_type: Option<&str>,
    query: &Query<'_>,
    elapsed: &mut dyn FnMut() -> Duration,
) -> Suggestions<'a> {
    let wanted = match entry_type {
        None => None,
        Some(id) => match index.types.iter().position(|(kind, _)| kind == id) {
            Some(kind) => Some(kind),
            None => {
                return Suggestions {
                    items: Vec::new(),
                    more: false,
                    cut: false,
                };
            }
        },
    };
    let eligible = |entry: &&Entry| !entry.retired && wanted.is_none_or(|kind| entry.kind == kind);
    let order =
        |a: &&Entry, b: &&Entry| (&a.norm, &a.name, &a.key).cmp(&(&b.norm, &b.name, &b.key));
    let limit = MAX_SUGGESTIONS + 1;
    let mut picked: Vec<&Entry> = Vec::new();
    let tier = |mut matches: Vec<&'a Entry>, picked: &mut Vec<&'a Entry>| {
        matches.sort_by(order);
        picked.extend(matches.into_iter().take(limit - picked.len()));
    };
    let candidates: Vec<&Entry> = index.entries.iter().filter(eligible).collect();
    tier(
        candidates
            .iter()
            .copied()
            .filter(|entry| entry.norm.starts_with(query.norm))
            .collect(),
        &mut picked,
    );
    if picked.len() < limit {
        tier(
            candidates
                .iter()
                .copied()
                .filter(|entry| {
                    !entry.norm.starts_with(query.norm) && entry.norm.contains(query.norm)
                })
                .collect(),
            &mut picked,
        );
    }
    let mut cut = false;
    if query.typo && picked.len() < limit {
        let distance = allowed(query.scalars);
        let mut near = Vec::new();
        for entry in candidates
            .iter()
            .copied()
            .filter(|entry| !entry.norm.contains(query.norm))
            .filter(|entry| entry.scalars.abs_diff(query.scalars) <= 2)
        {
            if elapsed() >= TYPO_BUDGET {
                cut = true;
                break;
            }
            if strsim::osa_distance(&entry.norm, query.norm) <= distance {
                near.push(entry);
            }
        }
        tier(near, &mut picked);
    }
    let more = picked.len() > MAX_SUGGESTIONS;
    picked.truncate(MAX_SUGGESTIONS);
    let items = picked
        .into_iter()
        .map(|entry| Found {
            entry,
            duplicate: candidates
                .iter()
                .filter(|other| other.norm == entry.norm)
                .nth(1)
                .is_some(),
        })
        .collect();
    Suggestions { items, more, cut }
}

#[cfg(test)]
#[path = "suggest_tests.rs"]
mod tests;

//! Catalogue reading through a private sheet (catalogue plan §3 step 3: D11, R6, R25).
//!
//! A catalogue is reached only through a sheet the reader may read: its immutable runtime pin
//! must link the catalogue by id, within the pin's own game and edition, and the latest sealed
//! version is read. Native pins, unsupported pins, unlinked, unknown or unsealed catalogues and
//! unreadable sheets all answer `NotFound`, which the routes render as one uniform refusal, so
//! they cannot be used to probe what exists or which check failed. Only once every check has
//! passed (the reader is inside a catalogue it may browse) does an unknown entry key or entry
//! type become [`Body::NoEntry`] or [`Body::NoType`] (plan §19): that reveals nothing the
//! browse page does not already show.
//!
//! Phases, all under the caller's runtime permit (R6):
//! 1. transaction A: recheck the session, authorize the read, load the raw snapshot and pin;
//! 2. outside the database mutex: decode the snapshot, validate the pin (write-only: nothing
//!    is evaluated), match it, and check the link;
//! 3. transaction B: recheck and authorize again, reload the sheet and require it to
//!    correspond to the validated snapshot, check the pin row unchanged, then the bounded
//!    catalogue lookups;
//! 4. render (the route).
use super::{
    sheets::match_pin,
    snapshots::{self, Pin},
};
use crate::{
    AppError, Store,
    auth::RequestAuth,
    catalogues::read::{self, BrowsePage, BrowseQuery, EntryPage},
    definitions::private_pin,
};
use tokio::sync::OwnedSemaphorePermit;

/// What a catalogue route asks for.
pub(crate) enum Request {
    Browse(BrowseQuery),
    Entry(String),
}

pub(crate) enum Body {
    Browse(BrowsePage),
    Entry(EntryPage),
    /// The query could not run (no searchable words, control characters, more than 16 pieces,
    /// or the time budget); the static message is shown with the search form.
    Problem(&'static str),
    /// No entry with this key in the latest sealed version.
    NoEntry(String),
    /// The type filter names no entry type of the latest sealed version.
    NoType(String),
}

pub(crate) struct CataloguePage {
    pub sheet_id: i64,
    pub catalogue: String,
    pub title: String,
    pub body: Body,
}

/// A catalogue link shown on the sheet page: the id, and the latest sealed title (`None`
/// when the catalogue has no sealed version any more or its title is damaged; only damage
/// can cause either, and the page then shows the link as unavailable).
pub(crate) struct LinkView {
    pub id: String,
    pub title: Option<String>,
}

/// Runs between phases 2 and 3 (tests only).
#[cfg(test)]
pub(super) static AFTER_CATALOGUE_VALIDATION: std::sync::Mutex<
    Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
> = std::sync::Mutex::new(None);

impl Store {
    pub(crate) async fn private_catalogue(
        &self,
        auth: RequestAuth,
        sheet: i64,
        catalogue: String,
        request: Request,
        permit: OwnedSemaphorePermit,
    ) -> Result<(CataloguePage, OwnedSemaphorePermit), AppError> {
        let first = auth.clone();
        let (raw, pin, permit) = self
            .run_bounded_auth(move |db| {
                let tx = db.transaction()?;
                first.recheck(&tx)?;
                snapshots::authorize_read(&tx, &first, sheet)?;
                let raw = snapshots::raw_load(&tx, sheet, None)?;
                let pin = match raw.identity.pin {
                    Pin::Runtime {
                        version_id,
                        schema: 1,
                        ..
                    } if snapshots::supported(&raw.identity) => private_pin::load(&tx, version_id)?,
                    _ => return Err(AppError::NotFound),
                };
                first.recheck(&tx)?;
                tx.commit()?;
                Ok((raw, pin, permit))
            })
            .await?;
        let (snapshot, pin, canonical, hash, permit) = tokio::task::spawn_blocking(move || {
            let snapshot = raw.decode()?;
            let pin = private_pin::validate_for_write(pin)?;
            match_pin(&snapshot, &pin)?;
            if !pin
                .definition()
                .catalogues
                .iter()
                .any(|link| link.id == catalogue)
            {
                return Err(AppError::NotFound);
            }
            let canonical = serde_json::to_string(&snapshot)?;
            let hash = snapshots::hash(canonical.as_bytes());
            #[cfg(test)]
            if let Some(hook) = AFTER_CATALOGUE_VALIDATION.lock().unwrap().take() {
                hook();
            }
            Ok::<_, AppError>((snapshot, pin, canonical, hash, (catalogue, permit)))
        })
        .await
        .map_err(|_| AppError::Worker)??;
        let (catalogue, permit) = permit;
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck(&tx)?;
            snapshots::authorize_read(&tx, &auth, sheet)?;
            let current = snapshots::raw_load(&tx, sheet, None)?;
            if !current.corresponds(&snapshot, &canonical, &hash)? {
                return Err(AppError::Conflict);
            }
            pin.unchanged(&tx)?;
            let linked =
                read::latest_sealed(&tx, &snapshot.game_id, &snapshot.edition_id, &catalogue)?
                    .ok_or(AppError::NotFound)?;
            let body = match request {
                Request::Browse(query) => match read::browse(&tx, &linked, &query) {
                    Ok(page) => Body::Browse(page),
                    Err(read::BrowseFailure::Query(message)) => Body::Problem(message),
                    Err(read::BrowseFailure::UnknownType) => {
                        Body::NoType(query.entry_type.unwrap_or_default())
                    }
                    Err(read::BrowseFailure::Other(error)) => return Err(error),
                },
                Request::Entry(key) => match read::entry(&tx, &linked, &key)? {
                    Some(entry) => Body::Entry(entry),
                    None => Body::NoEntry(key),
                },
            };
            auth.recheck(&tx)?;
            tx.commit()?;
            Ok((
                CataloguePage {
                    sheet_id: sheet,
                    // A damaged title degrades to the catalogue id.
                    title: linked.title.unwrap_or_else(|| catalogue.clone()),
                    catalogue,
                    body,
                },
                permit,
            ))
        })
        .await
    }
}

/// The sheet page's catalogue links, read inside its final read boundary.
pub(super) fn links(
    db: &rusqlite::Connection,
    pin: &private_pin::ImmutablePin,
) -> Result<Vec<LinkView>, AppError> {
    let definition = pin.definition();
    definition
        .catalogues
        .iter()
        .map(|link| {
            Ok(LinkView {
                id: link.id.clone(),
                title: read::latest_sealed(
                    db,
                    &definition.game_id,
                    &definition.edition_id,
                    &link.id,
                )?
                .and_then(|linked| linked.title),
            })
        })
        .collect()
}

/// A saved row's link to a catalogue entry (assisted sheet step 3a, linking rules 1 and 2):
/// the catalogue's place in the definition's links (which names the link on the page through
/// the Catalogues panel) and its id, and the entry key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EntryLink {
    pub index: usize,
    pub catalogue: String,
    pub key: String,
}

impl EntryLink {
    /// The entry page of the sheet's catalogue route.
    pub(crate) fn href(&self, sheet: i64) -> String {
        format!(
            "/private-sheets/{sheet}/catalogue/{}/{}",
            self.catalogue, self.key
        )
    }
}

/// Links of the saved rows, by `(list field index, row host ID)`.
pub(crate) type EntryLinks = std::collections::BTreeMap<(usize, String), EntryLink>;

/// Linking on view for the saved `data` of a sheet pinned to `pin`: for each saved row of a
/// list with a `catalogue` member and a non-blank name, (1) the entry the stored key property
/// names, if it is an eligible entry of the latest sealed version whose normalised name equals
/// the cell's, else (2) the single eligible entry with that name; none or several: no link.
/// Eligible means not retired and of the member's entry type, when it has one. One query per
/// list under `QUERY_BUDGET`; a failure or timeout gives no links for that list and logs
/// nothing about names. The catalogue must be reachable the way the catalogue route would
/// serve it (a sealed version), which the caller's supported runtime pin already guarantees.
pub(super) fn entry_links(
    db: &rusqlite::Connection,
    pin: &private_pin::ImmutablePin,
    data: &serde_json::Value,
) -> EntryLinks {
    entry_links_within(db, pin, data, (crate::sources::search::QUERY_BUDGET, None))
}

/// `entry_links` with its budget and, for tests that must interrupt very short reads, the
/// VM-instruction interval of the budget check.
pub(super) fn entry_links_within(
    db: &rusqlite::Connection,
    pin: &private_pin::ImmutablePin,
    data: &serde_json::Value,
    budget: (std::time::Duration, Option<i32>),
) -> EntryLinks {
    let definition = pin.definition();
    let mut links = EntryLinks::new();
    for (f, field) in definition.fields.iter().enumerate() {
        let properties = super::values::properties(field);
        let (Some(name), Some(key)) = (
            properties
                .iter()
                .position(|property| property.catalogue.is_some()),
            super::values::key_property(field),
        ) else {
            continue;
        };
        let Some(member) = properties[name].catalogue.as_ref() else {
            continue;
        };
        let Some(index) = definition
            .catalogues
            .iter()
            .position(|link| link.id == member.id)
        else {
            continue;
        };
        let rows = data[&field.id].as_array().map_or(&[][..], Vec::as_slice);
        let outcome = rows_links(
            db,
            pin,
            (index, member),
            (&properties[name].id, &properties[key].id),
            rows,
            budget,
        );
        match outcome {
            Ok(found) => links.extend(found.into_iter().map(|(host, link)| ((f, host), link))),
            Err(_) => eprintln!("Mimik: catalogue links unavailable for one list"),
        }
    }
    links
}

fn rows_links(
    db: &rusqlite::Connection,
    pin: &private_pin::ImmutablePin,
    (index, member): (usize, &crate::definitions::CatalogueMember),
    (name, key): (&str, &str),
    rows: &[serde_json::Value],
    budget: (std::time::Duration, Option<i32>),
) -> Result<Vec<(String, EntryLink)>, AppError> {
    let definition = pin.definition();
    let Some(linked) =
        read::latest_sealed(db, &definition.game_id, &definition.edition_id, &member.id)?
    else {
        return Ok(Vec::new());
    };
    let named: Vec<(&str, String, Option<&str>)> = rows
        .iter()
        .filter_map(|row| {
            let norm = crate::text_fold::name_norm(row[name].as_str()?);
            (!norm.is_empty()).then_some((row["id"].as_str()?, norm, row[key].as_str()))
        })
        .collect();
    if named.is_empty() {
        return Ok(Vec::new());
    }
    let mut wanted: Vec<String> = named.iter().map(|(_, norm, _)| norm.clone()).collect();
    wanted.sort();
    wanted.dedup();
    let found = {
        let _budget = match budget.1 {
            Some(ops) => crate::sources::search::Budget::checking_every(db, budget.0, ops),
            None => crate::sources::search::Budget::start(db, budget.0),
        };
        crate::catalogues::names::eligible(
            db,
            linked.version_id,
            member.entry_type.as_deref(),
            &wanted,
        )
    }
    .map_err(AppError::from)?;
    let link = |entry: &str| EntryLink {
        index,
        catalogue: member.id.clone(),
        key: entry.to_owned(),
    };
    let mut by_norm: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (norm, entry) in &found {
        by_norm.entry(norm).or_default().push(entry);
    }
    Ok(named
        .into_iter()
        .filter_map(|(host, norm, stored)| {
            let matching = by_norm.get(norm.as_str())?;
            let chosen = stored
                .and_then(|stored| matching.iter().copied().find(|entry| *entry == stored))
                .or_else(|| (matching.len() == 1).then(|| matching[0]))?;
            Some((host.to_owned(), link(chosen)))
        })
        .collect())
}

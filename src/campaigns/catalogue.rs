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

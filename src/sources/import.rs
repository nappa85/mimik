use super::sanitized;
use crate::local_file::{flags as open_flags, read as read_file};
use crate::{AppError, Store};
use rusqlite::{OptionalExtension, params};
use rustix::fs::{Mode, OFlags};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::Path,
};

const MIB: usize = 1024 * 1024;
// Match the publication evidence metadata bound (UTF-8 bytes, not characters).
const MAX_BLOCK_ID_BYTES: usize = 512;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    contract_version: u32,
    source_id: String,
    title: String,
    system_id: String,
    edition: String,
    #[serde(default)]
    edition_id: EditionIdentity,
    language: String,
    source_sha256: String,
    extraction_sha256: String,
    extractor: Provenance,
    renderer: Provenance,
    pages: Vec<PreparedPage>,
}

#[derive(Default)]
enum EditionIdentity {
    #[default]
    Missing,
    Null,
    Id(String),
}

impl<'de> Deserialize<'de> for EditionIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        Ok(Option::<String>::deserialize(decoder)?.map_or(Self::Null, Self::Id))
    }
}

impl Bundle {
    fn stable_edition(&self) -> Result<Option<&str>, AppError> {
        match (self.contract_version, &self.edition_id) {
            (1, EditionIdentity::Missing) => Ok(None),
            (2, EditionIdentity::Id(id)) if crate::registry::valid_id(id) => Ok(Some(id)),
            _ => Err(invalid()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedPage {
    pdf_page: u32,
    width_points: f64,
    height_points: f64,
    image_sha256: String,
    units: Vec<Unit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    name: String,
    version: String,
    settings: serde_json::Value,
}

impl Provenance {
    fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 100
            && !self.version.trim().is_empty()
            && self.version.len() <= 200
            && self.settings.is_object()
    }
}

#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Unit {
    id: String,
    kind: String,
    polygon: Vec<[f64; 2]>,
    html: String,
}

#[derive(Deserialize)]
struct RawNode {
    id: String,
    block_type: String,
    html: String,
    polygon: Vec<[f64; 2]>,
    children: Option<Vec<RawNode>>,
}

#[derive(Deserialize)]
struct RawDocument {
    children: Vec<RawNode>,
}

fn invalid() -> AppError {
    AppError::Invalid(
        "Invalid, unsupported or inconsistent source bundle. See the prepared-bundle contract.",
    )
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn artifact(dir: &rustix::fd::OwnedFd, name: &str, limit: usize) -> Result<Vec<u8>, AppError> {
    read_file(
        rustix::fs::openat(dir, name, open_flags(), Mode::empty()).map_err(std::io::Error::from)?,
        limit,
    )
}

fn pdf_number(value: &lopdf::Object) -> Result<f64, AppError> {
    match value {
        lopdf::Object::Integer(value) => {
            Ok(f64::from(i32::try_from(*value).map_err(|_| invalid())?))
        }
        lopdf::Object::Real(value) => Ok(f64::from(*value)),
        _ => Err(invalid()),
    }
}

fn inherited(
    doc: &lopdf::Document,
    mut id: lopdf::ObjectId,
    key: &[u8],
) -> Result<Option<lopdf::Object>, AppError> {
    let mut seen = BTreeSet::new();
    for _ in 0..32 {
        if !seen.insert(id) {
            return Err(invalid());
        }
        let dict = doc.get_dictionary(id).map_err(|_| invalid())?;
        if let Ok(value) = dict.get(key) {
            return Ok(Some(value.clone()));
        }
        let Ok(parent) = dict.get(b"Parent") else {
            return Ok(None);
        };
        id = parent.as_reference().map_err(|_| invalid())?;
    }
    Err(invalid())
}

fn pdf_bounds(doc: &lopdf::Document, id: lopdf::ObjectId) -> Result<(f64, f64), AppError> {
    if let Some(rotation) = inherited(doc, id, b"Rotate")?
        && pdf_number(&rotation)? != 0.0
    {
        return Err(invalid());
    }
    let bounds = inherited(doc, id, b"CropBox")?
        .or(inherited(doc, id, b"MediaBox")?)
        .ok_or_else(invalid)?;
    let array = bounds.as_array().map_err(|_| invalid())?;
    if array.len() != 4 {
        return Err(invalid());
    }
    let values = array
        .iter()
        .map(pdf_number)
        .collect::<Result<Vec<_>, _>>()?;
    if values[0] != 0.0 || values[1] != 0.0 {
        return Err(invalid());
    }
    Ok((values[2], values[3]))
}

fn collect_units(
    node: &RawNode,
    page: u32,
    seen: &mut BTreeSet<String>,
    units: &mut BTreeMap<String, Unit>,
    depth: usize,
) -> Result<(), AppError> {
    if depth > 32
        || seen.len() >= 40_000
        || !seen.insert(node.id.clone())
        || !node.id.starts_with(&format!("/page/{}/", page - 1))
    {
        return Err(invalid());
    }
    let children = node.children.as_deref().unwrap_or_default();
    let eligible = super::review_kind(&node.block_type)?;
    if eligible {
        if !children.is_empty() || node.html.contains("<content-ref") || node.html.len() > 64 * 1024
        {
            return Err(invalid());
        }
        units.insert(
            node.id.clone(),
            Unit {
                id: node.id.clone(),
                kind: node.block_type.clone(),
                polygon: node.polygon.clone(),
                html: node.html.clone(),
            },
        );
    }
    for child in children {
        collect_units(child, page, seen, units, depth + 1)?;
    }
    Ok(())
}

fn validate_page(
    page: &PreparedPage,
    raw: &RawNode,
    doc: &lopdf::Document,
    id: lopdf::ObjectId,
) -> Result<(), AppError> {
    if !(1.0..=4_000.0).contains(&page.width_points)
        || !(1.0..=4_000.0).contains(&page.height_points)
        || raw.block_type != "Page"
    {
        return Err(invalid());
    }
    let (width, height) = pdf_bounds(doc, id)?;
    if !(1.0..=4_000.0).contains(&width)
        || !(1.0..=4_000.0).contains(&height)
        || (width - page.width_points).abs() > 2.0
        || (height - page.height_points).abs() > 2.0
        || raw.polygon
            != vec![
                [0.0, 0.0],
                [page.width_points, 0.0],
                [page.width_points, page.height_points],
                [0.0, page.height_points],
            ]
    {
        return Err(invalid());
    }
    let mut units = BTreeMap::new();
    collect_units(raw, page.pdf_page, &mut BTreeSet::new(), &mut units, 0)?;
    if units.len() != page.units.len() || units.len() > 20_000 {
        return Err(invalid());
    }
    for unit in &page.units {
        if unit.id.len() > MAX_BLOCK_ID_BYTES
            || units.remove(&unit.id).as_ref() != Some(unit)
            || unit.polygon.len() != 4
            || unit.polygon.iter().any(|[x, y]| {
                !x.is_finite()
                    || !y.is_finite()
                    || *x < 0.0
                    || *y < 0.0
                    || *x > page.width_points
                    || *y > page.height_points
            })
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn validate_image(image: &[u8], page: &PreparedPage) -> Result<(), AppError> {
    let mut decoder = png::Decoder::new(Cursor::new(image));
    decoder.set_limits(png::Limits { bytes: 64 * MIB });
    let mut reader = decoder.read_info().map_err(|_| invalid())?;
    validate_image_header(reader.info(), page.width_points, page.height_points)?;
    let size = reader.output_buffer_size().ok_or_else(invalid)?;
    if size > 64 * MIB {
        return Err(invalid());
    }
    let mut pixels = vec![0; size];
    reader.next_frame(&mut pixels).map_err(|_| invalid())?;
    Ok(())
}

pub(super) fn validate_image_header(
    info: &png::Info<'_>,
    width_points: f64,
    height_points: f64,
) -> Result<(), AppError> {
    if info.animation_control.is_some()
        || info.width == 0
        || info.height == 0
        || info.width > 4_096
        || info.height > 8_192
        || u64::from(info.width) * u64::from(info.height) > 12_000_000
        || !(1.0..=4_000.0).contains(&width_points)
        || !(1.0..=4_000.0).contains(&height_points)
        || (f64::from(info.width) / f64::from(info.height) - width_points / height_points).abs()
            > 0.01
    {
        return Err(invalid());
    }
    Ok(())
}

struct Import {
    bundle: Bundle,
    prepared: Vec<u8>,
    pdf: Vec<u8>,
    raw: Vec<u8>,
    images: Vec<Vec<u8>>,
    total_pages: i64,
    units: Vec<Vec<StoredUnit>>,
}

struct StoredUnit {
    block_id: String,
    kind: String,
    polygon: String,
    html: String,
    text: String,
    search: String,
}

fn normalized(page: &PreparedPage) -> Result<Vec<StoredUnit>, AppError> {
    page.units
        .iter()
        .map(|unit| {
            let clean = sanitized(&unit.html)?;
            let text = html2text::from_read(clean.as_bytes(), 120).map_err(|_| invalid())?;
            if text.len() > 256 * 1024 {
                return Err(invalid());
            }
            Ok(StoredUnit {
                block_id: unit.id.clone(),
                kind: unit.kind.clone(),
                polygon: serde_json::to_string(&unit.polygon)?,
                html: unit.html.clone(),
                text,
                search: super::search::search_text(&unit.html)?,
            })
        })
        .collect()
}

fn verify_existing(db: &rusqlite::Connection, id: i64, data: &Import) -> Result<(), AppError> {
    let source = &data.bundle;
    let matches: bool = db.query_row("SELECT source_id=?2 AND title=?3 AND system_id=?4 AND edition=?5 AND language=?6 AND pdf_hash=?7 AND pdf=?8 AND raw_json=?9 AND prepared_json=?10 AND bundle_hash=?11 FROM sources WHERE id=?1", params![id,source.source_id,source.title,source.system_id,source.edition,source.language,source.source_sha256,data.pdf,data.raw,data.prepared,hash(&data.prepared)], |row| row.get(0))?;
    if !matches {
        return Err(invalid());
    }
    let pages: usize = db.query_row(
        "SELECT count(*) FROM source_pages WHERE source_id=?1",
        [id],
        |row| row.get(0),
    )?;
    if pages != source.pages.len() {
        return Err(invalid());
    }
    for ((page, image), units) in source.pages.iter().zip(&data.images).zip(&data.units) {
        let (page_id, same_image): (i64, bool) = db
            .query_row(
                "SELECT id,png=?3 FROM source_pages WHERE source_id=?1 AND pdf_page=?2",
                params![id, page.pdf_page, image],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(invalid)?;
        if !same_image {
            return Err(invalid());
        }
        let count: usize = db.query_row(
            "SELECT count(*) FROM source_units WHERE page_id=?1",
            [page_id],
            |row| row.get(0),
        )?;
        if count != units.len() {
            return Err(invalid());
        }
        for unit in units {
            let matches: bool = db.query_row("SELECT kind=?3 AND polygon=?4 AND raw_html=?5 AND raw_text=?6 FROM source_units WHERE page_id=?1 AND block_id=?2", params![page_id,unit.block_id,unit.kind,unit.polygon,unit.html,unit.text], |row| row.get(0)).optional()?.ok_or_else(invalid)?;
            if !matches {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn finalize(db: &rusqlite::Connection, id: i64, data: &Import) -> Result<(), AppError> {
    let existing: Option<(String, String)> = db
        .query_row(
            "SELECT prepared_hash,extraction_hash FROM source_finalizations WHERE source_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let expected = (hash(&data.prepared), data.bundle.extraction_sha256.clone());
    let finalized = existing.is_some();
    if existing.is_some_and(|hashes| hashes != expected) {
        return Err(invalid());
    }
    for page in &data.bundle.pages {
        let page_id: i64 = db.query_row(
            "SELECT id FROM source_pages WHERE source_id=?1 AND pdf_page=?2",
            params![id, page.pdf_page],
            |row| row.get(0),
        )?;
        if finalized {
            let same: bool = db.query_row("SELECT source_id=?2 AND image_hash=?3 FROM source_page_hashes WHERE page_id=?1", params![page_id,id,page.image_sha256], |row| row.get(0)).optional()?.ok_or_else(invalid)?;
            if !same {
                return Err(invalid());
            }
        } else {
            db.execute(
                "INSERT INTO source_page_hashes VALUES (?1,?2,?3)",
                params![page_id, id, page.image_sha256],
            )?;
        }
    }
    let count: usize = db.query_row(
        "SELECT count(*) FROM source_page_hashes WHERE source_id=?1",
        [id],
        |row| row.get(0),
    )?;
    if count != data.bundle.pages.len() {
        return Err(invalid());
    }
    if !finalized {
        db.execute(
            "INSERT INTO source_finalizations VALUES (?1,?2,?3)",
            params![id, expected.0, expected.1],
        )?;
    }
    Ok(())
}

fn load_import(directory: &Path, pdf_path: &Path) -> Result<Import, AppError> {
    let dir = rustix::fs::open(
        directory,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let prepared = artifact(&dir, "bundle.json", 8 * MIB)?;
    let bundle: Bundle = serde_json::from_slice(&prepared)?;
    bundle.stable_edition()?;
    if bundle.pages.is_empty()
        || bundle.pages.len() > 64
        || bundle.title.trim().is_empty()
        || bundle.title.len() > 200
        || bundle.source_id.trim().is_empty()
        || bundle.source_id.len() > 100
        || bundle.edition.trim().is_empty()
        || (bundle.contract_version == 1 && bundle.edition.len() > 100)
        || (bundle.contract_version == 2 && bundle.edition.chars().count() > 100)
        || bundle.language.trim().is_empty()
        || bundle.language.len() > 30
        || !crate::registry::valid_id(&bundle.system_id)
        || !bundle.extractor.valid()
        || !bundle.renderer.valid()
    {
        return Err(invalid());
    }
    let pdf = read_file(
        rustix::fs::open(pdf_path, open_flags(), Mode::empty()).map_err(std::io::Error::from)?,
        128 * MIB,
    )?;
    let raw = artifact(&dir, "source.json", 32 * MIB)?;
    if hash(&pdf) != bundle.source_sha256 || hash(&raw) != bundle.extraction_sha256 {
        return Err(invalid());
    }
    let document = lopdf::Document::load_mem(&pdf).map_err(|_| invalid())?;
    let pdf_pages = document.get_pages();
    let total_pages = i64::try_from(pdf_pages.len()).map_err(|_| invalid())?;
    if total_pages == 0 {
        return Err(invalid());
    }
    let raw_document: RawDocument = serde_json::from_slice(&raw)?;
    if raw_document.children.len() != bundle.pages.len() {
        return Err(invalid());
    }
    let mut images = Vec::new();
    let mut seen = BTreeSet::new();
    let mut image_bytes = 0;
    let mut unit_count = 0;
    let mut units = Vec::new();
    for page in &bundle.pages {
        if page.pdf_page == 0 || !seen.insert(page.pdf_page) {
            return Err(invalid());
        }
        let id = *pdf_pages.get(&page.pdf_page).ok_or_else(invalid)?;
        let prefix = format!("/page/{}/Page/", page.pdf_page - 1);
        let nodes = raw_document
            .children
            .iter()
            .filter(|raw| raw.id.starts_with(&prefix))
            .collect::<Vec<_>>();
        if nodes.len() != 1 {
            return Err(invalid());
        }
        validate_page(page, nodes[0], &document, id)?;
        unit_count += page.units.len();
        if unit_count > 20_000 {
            return Err(invalid());
        }
        let image = artifact(&dir, &format!("page-{}.png", page.pdf_page), 12 * MIB)?;
        image_bytes += image.len();
        if image_bytes > 128 * MIB {
            return Err(invalid());
        }
        if hash(&image) != page.image_sha256 {
            return Err(invalid());
        }
        validate_image(&image, page)?;
        images.push(image);
        units.push(normalized(page)?);
    }
    Ok(Import {
        bundle,
        prepared,
        pdf,
        raw,
        images,
        total_pages,
        units,
    })
}

impl Store {
    /// Register a prepared local source directory. Incoming approvals are never used.
    ///
    /// # Errors
    /// Rejects inconsistent/unsupported artifacts, bounds, hashes, symlinks, sizes or storage errors.
    pub async fn import_source(&self, directory: &Path, pdf: &Path) -> Result<i64, AppError> {
        let directory = directory.to_owned();
        let pdf = pdf.to_owned();
        let data = tokio::task::spawn_blocking(move || load_import(&directory, &pdf))
            .await
            .map_err(|_| AppError::Worker)??;
        self.run(move |db| {
            let tx = db.transaction()?;
            let resolved = crate::registry::source_identity(&tx, &data.bundle.system_id, &data.bundle.edition, data.bundle.stable_edition()?)?;
            let bundle_hash = hash(&data.prepared);
            let existing = tx.query_row("SELECT id,pdf_hash,total_pages FROM sources WHERE bundle_hash = ?1", [&bundle_hash], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?))).optional()?;
            if let Some((id, pdf_hash, total_pages)) = existing {
                let current = tx.query_row("SELECT game_id,edition_id FROM source_rulesets WHERE source_id=?1", [id], |row| Ok(crate::registry::RulesetKey { game_id: row.get(0)?, edition_id: row.get(1)? })).optional()?;
                if current != resolved { return Err(AppError::Invalid("Existing source association disagrees with import identity; no data was changed.")); }
                if pdf_hash != data.bundle.source_sha256 || (total_pages != 0 && total_pages != data.total_pages) { return Err(invalid()); }
                verify_existing(&tx, id, &data)?;
                if total_pages == 0 { tx.execute("UPDATE sources SET total_pages=?1 WHERE id=?2 AND total_pages=0", params![data.total_pages,id])?; }
                finalize(&tx, id, &data)?;
                tx.commit()?;
                return Ok(id);
            }
            let source = &data.bundle;
            tx.execute("INSERT INTO sources (bundle_hash, source_id, title, system_id, edition, language, pdf_hash, pdf, raw_json, prepared_json,total_pages) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![bundle_hash, source.source_id, source.title, source.system_id, source.edition, source.language, source.source_sha256, data.pdf, data.raw, data.prepared,data.total_pages])?;
            let id = tx.last_insert_rowid();
            if let Some(key) = resolved {
                tx.execute("INSERT INTO source_rulesets VALUES (?1,?2,?3)", params![id,key.game_id,key.edition_id])?;
            }
            for ((page, image), units) in source.pages.iter().zip(&data.images).zip(&data.units) {
                tx.execute("INSERT INTO source_pages (source_id, pdf_page, png) VALUES (?1,?2,?3)", params![id, page.pdf_page, image])?;
                let page_id = tx.last_insert_rowid();
                for unit in units {
                    tx.execute("INSERT INTO source_units (page_id, block_id, kind, polygon, raw_html, raw_text) VALUES (?1,?2,?3,?4,?5,?6)", params![page_id, unit.block_id, unit.kind, unit.polygon, unit.html, unit.text])?;
                    tx.execute("INSERT INTO source_search_text (unit_id,normaliser,search_text) VALUES (?1,?2,?3)", params![tx.last_insert_rowid(), super::search::SEARCH_NORMALISER, unit.search])?;
                }
            }
            finalize(&tx, id, &data)?;
            tx.commit()?;
            Ok(id)
        }).await
    }
}

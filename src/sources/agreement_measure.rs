//! Prototype-tier measurement of the agreement rule on a copy of the Owner's database
//! (`docs/results.md`, agreement-v1). Ignored by default; set `MIMIK_AGREEMENT_DB` to a
//! SQLite file. The file is opened read-only and immutable, never migrated and never written.
//! Prints aggregates only: counts, unit ids, page numbers. No unit, word, correction or
//! rationale text is ever printed.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::agreement::{Exclusion, UnitInput, Verdict, classify_provenance, evaluate, unit_tokens};
use super::materials::ORDERED_UNITS;
use super::page_words::{self, PageWord};
use super::proposals::{self, Candidate};
use super::shapes::{self, ShapeInput};
use super::table_words;

const TEXT_BUDGET: usize = 65_536;
const MIN_SCORE: u32 = 80;

type Counts = BTreeMap<String, usize>;

fn bump(counts: &mut Counts, key: impl Into<String>) {
    *counts.entry(key.into()).or_default() += 1;
}

fn label(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Agrees => "Agrees".into(),
        Verdict::Differs { reason, .. } => format!("Differs({reason:?})"),
        Verdict::NoWords => "NoWords".into(),
        Verdict::Excluded(reason) => format!("Excluded({reason:?})"),
    }
}

fn print_counts(title: &str, counts: &Counts) {
    let body: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!(
        "  {title}: {}",
        if body.is_empty() {
            "-".into()
        } else {
            body.join(" ")
        }
    );
}

struct Unit {
    id: i64,
    kind: String,
    origin: String,
    state: String,
    polygon: String,
    raw_text: String,
    correction: String,
}

/// Whether two texts differ only in whitespace and case, compared after the rule's own
/// markup removal, NFC and folds (so a sign, a dash or any other character that the rule keeps
/// is a difference).
fn whitespace_or_case_only(original: &str, correction: &str) -> bool {
    let squeeze = |text: &str| unit_tokens(text).0.concat().to_lowercase();
    squeeze(original) == squeeze(correction)
}

fn open() -> Connection {
    let path = std::env::var("MIMIK_AGREEMENT_DB").expect("MIMIK_AGREEMENT_DB is not set");
    let escaped = path
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23");
    Connection::open_with_flags(
        format!("file:{escaped}?immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("open read-only")
}

/// Open suggestions of a source: unit targets and pages with a page-level one.
fn suggestions(db: &Connection, source: i64) -> (HashSet<i64>, HashSet<i64>, usize, usize) {
    let mut query = db
        .prepare(
            "SELECT s.kind,s.targets,EXISTS(SELECT 1 FROM source_suggestion_applications a WHERE a.suggestion_id=s.id),EXISTS(SELECT 1 FROM source_suggestion_dismissals d WHERE d.suggestion_id=s.id) FROM source_suggestions s WHERE s.source_id=?1",
        )
        .unwrap();
    let rows = query
        .query_map([source], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })
        .unwrap();
    let (mut units, mut pages, mut open, mut total) = (HashSet::new(), HashSet::new(), 0, 0);
    for row in rows {
        let (kind, targets, applied, dismissed) = row.unwrap();
        total += 1;
        // A stale suggestion (its unit changed since) is kept as open: the stricter reading.
        if applied || dismissed {
            continue;
        }
        open += 1;
        let target: Value = serde_json::from_str(&targets).unwrap_or(Value::Null);
        let page = target["page"].as_i64();
        match target["type"].as_str() {
            Some("unit") => {
                units.extend(target["target"]["unit"].as_i64());
            }
            Some("units") => units.extend(
                target["targets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["unit"].as_i64()),
            ),
            Some("region" | "page") if kind == "missing_text" || kind == "note" => {
                pages.extend(page);
            }
            _ => {}
        }
    }
    (units, pages, open, total)
}

fn units_of(db: &Connection, page: i64) -> Vec<Unit> {
    let mut query = db
        .prepare(&format!(
            "{ORDERED_UNITS} SELECT id,kind,origin,state,polygon,raw_text,correction FROM ordered ORDER BY position"
        ))
        .unwrap();
    query
        .query_map([page], |r| {
            Ok(Unit {
                id: r.get(0)?,
                kind: r.get(1)?,
                origin: r.get(2)?,
                state: r.get(3)?,
                polygon: r.get(4)?,
                raw_text: r.get(5)?,
                correction: r.get(6)?,
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The text each candidate unit contributes, within the proposals' page budget; `None` past it
/// or when blank.
fn budgeted(units: &[Unit], extracted_only_unreviewed: bool) -> Vec<Option<String>> {
    let mut budget = TEXT_BUDGET;
    units
        .iter()
        .map(|u| {
            let candidate =
                u.origin == "extracted" && (!extracted_only_unreviewed || u.state == "unreviewed");
            if !candidate || u.raw_text.trim().is_empty() || u.raw_text.len() > TEXT_BUDGET {
                return None;
            }
            (u.raw_text.len() <= budget).then(|| {
                budget -= u.raw_text.len();
                u.raw_text.clone()
            })
        })
        .collect()
}

struct Page {
    number: i64,
    verdicts_a: Vec<(i64, Verdict)>,
    orphans_a: usize,
    verdicts_b: Vec<(i64, Verdict)>,
    /// Propose, shape scoring, evaluate (a), evaluate (b).
    times: [Duration; 4],
}

#[expect(clippy::too_many_lines, reason = "one linear measurement pass")]
fn run_page(
    units: &[Unit],
    words: &[PageWord],
    open_units: &HashSet<i64>,
    page_level: bool,
    number: i64,
    counters: &mut [usize; 2],
) -> Page {
    // (a) as is.
    let texts = budgeted(units, true);
    let candidates: Vec<Candidate> = units
        .iter()
        .zip(&texts)
        .map(|(u, text)| Candidate {
            id: u.id,
            polygon: u.polygon.clone(),
            rejected: u.state == "rejected",
            text: text.clone(),
        })
        .collect();
    let started = Instant::now();
    let in_proposal: HashSet<i64> = proposals::propose(&candidates, Some(words))
        .into_iter()
        .flat_map(|p| p.units)
        .collect();
    let propose_time = started.elapsed();
    let mut shape_time = Duration::ZERO;
    let inputs: Vec<UnitInput<'_>> = units
        .iter()
        .zip(&texts)
        .map(|(u, text)| {
            let started = Instant::now();
            let shaped = text.as_deref().is_some_and(|t| {
                shapes::best(&ShapeInput::new(&[t]))
                    .iter()
                    .any(|d| d.shape != shapes::Shape::Plain && d.score >= MIN_SCORE)
            });
            shape_time += started.elapsed();
            let candidate = u.origin == "extracted" && u.state == "unreviewed";
            let blank = u.raw_text.trim().is_empty();
            let proposed = candidate && in_proposal.contains(&u.id);
            // Totals, whatever else excludes the unit.
            counters[0] += usize::from(proposed);
            counters[1] += usize::from(candidate && shaped);
            let excluded = if !candidate || blank {
                None
            } else if page_level {
                Some(Exclusion::PageSuggestion)
            } else if open_units.contains(&u.id) {
                Some(Exclusion::OpenSuggestion)
            } else if proposed {
                Some(Exclusion::TableProposal)
            } else if shaped {
                Some(Exclusion::TableShape)
            } else if text.is_none() {
                Some(Exclusion::TextBudget)
            } else {
                None
            };
            UnitInput {
                id: u.id,
                kind: &u.kind,
                origin: &u.origin,
                state: &u.state,
                polygon: &u.polygon,
                text: text.as_deref().unwrap_or(""),
                excluded,
            }
        })
        .collect();
    let started = Instant::now();
    let a = evaluate(&inputs, words, &[]);
    let time_a = started.elapsed();
    assert!(a.computed || units.len() > 2000);

    // (b) as extracted: extracted units only, all unreviewed, original text, no suggestions.
    let extracted: Vec<&Unit> = units.iter().filter(|u| u.origin == "extracted").collect();
    let b_texts = {
        let mut budget = TEXT_BUDGET;
        extracted
            .iter()
            .map(|u| {
                if u.raw_text.len() <= budget {
                    budget -= u.raw_text.len();
                    u.raw_text.as_str()
                } else {
                    ""
                }
            })
            .collect::<Vec<_>>()
    };
    let b_inputs: Vec<UnitInput<'_>> = extracted
        .iter()
        .zip(&b_texts)
        .map(|(u, text)| UnitInput {
            id: u.id,
            kind: &u.kind,
            origin: "extracted",
            state: "unreviewed",
            polygon: &u.polygon,
            text,
            excluded: (text.is_empty() && !u.raw_text.trim().is_empty())
                .then_some(Exclusion::TextBudget),
        })
        .collect();
    let started = Instant::now();
    let b = evaluate(&b_inputs, words, &[]);
    let time_b = started.elapsed();
    Page {
        number,
        orphans_a: a.facts.orphans.len(),
        verdicts_a: a.verdicts.into_iter().map(|v| (v.id, v.verdict)).collect(),
        verdicts_b: b.verdicts.into_iter().map(|v| (v.id, v.verdict)).collect(),
        times: [propose_time, shape_time, time_a, time_b],
    }
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

#[test]
#[expect(clippy::too_many_lines, reason = "one linear measurement printout")]
#[ignore = "needs MIMIK_AGREEMENT_DB, a copy of the Owner's database"]
fn measure_agreement() {
    let db = open();
    let sources: Vec<(i64, Vec<u8>)> = db
        .prepare("SELECT id,prepared_json FROM sources ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    println!(
        "rule agreement-v1; sources={}; build={}",
        sources.len(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let table_operations: HashSet<String> = db
        .prepare("SELECT operation_id FROM source_owner_tables")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let upload_exclusions: i64 = db
        .query_row("SELECT count(*) FROM source_upload_exclusions", [], |r| {
            r.get(0)
        })
        .unwrap();
    println!(
        "exclusion boxes: not applied (they belong to staged uploads, not bundle sources; rows in source_upload_exclusions: {upload_exclusions})"
    );
    let mut max_times = [Duration::ZERO; 5];
    for (source, prepared) in sources {
        let prepared: Value = serde_json::from_slice(&prepared).unwrap_or(Value::Null);
        let extractor = &prepared["extractor"];
        let kind = classify_provenance(
            extractor["name"].as_str().unwrap_or(""),
            extractor["version"].as_str().unwrap_or(""),
            &extractor["settings"],
        );
        let (open_units, page_suggestions, open_n, total_n) = suggestions(&db, source);
        let pages: Vec<(i64, i64)> = db
            .prepare("SELECT id,pdf_page FROM source_pages WHERE source_id=?1 ORDER BY pdf_page")
            .unwrap()
            .query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        println!(
            "source {source}: reading={kind:?} pages={} suggestions={total_n} open={open_n}",
            pages.len()
        );
        let (mut no_row, mut invalid, mut counters) = (0, 0, [0_usize; 2]);
        let mut corrected_without_words = 0;
        let (mut rejected_by_table, mut rejected_by_table_agrees) = (0, 0);
        let mut source_times = [Duration::ZERO; 5];
        let mut by_state = Counts::new();
        let (mut a_verdicts, mut a_kinds, mut b_verdicts, mut b_kinds) =
            (Counts::new(), Counts::new(), Counts::new(), Counts::new());
        let mut reasons = Counts::new();
        let (mut orphans, mut orphan_pages, mut compared_pages) = (0, 0, 0);
        let mut safety: BTreeMap<String, Counts> = BTreeMap::new();
        let mut corrected_agrees: Vec<String> = Vec::new();
        let (mut raised_units, mut raised_words, mut agreeing_words) = (0, 0, 0);
        let mut unit_kinds: HashMap<i64, String> = HashMap::new();
        for (page_id, number) in pages {
            let units = units_of(&db, page_id);
            for u in &units {
                bump(&mut by_state, format!("{}/{}", u.state, u.origin));
                unit_kinds.insert(u.id, u.kind.clone());
            }
            let stored: Option<Vec<u8>> = db
                .query_row(
                    "SELECT words FROM source_page_words WHERE page_id=?1",
                    [page_id],
                    |r| r.get(0),
                )
                .ok();
            let corrected_here = units
                .iter()
                .filter(|u| u.state == "corrected" && u.origin == "extracted")
                .count();
            let Some(stored) = stored else {
                no_row += 1;
                corrected_without_words += corrected_here;
                continue;
            };
            let started = Instant::now();
            let decoded = page_words::decode(&stored);
            source_times[4] = source_times[4].max(started.elapsed());
            let Ok(words) = decoded else {
                invalid += 1;
                corrected_without_words += corrected_here;
                continue;
            };
            let rejecting: HashMap<i64, String> = db
                .prepare("SELECT unit_id,operation_id FROM source_review_events WHERE page_id=?1 AND state='rejected' AND unit_id IS NOT NULL AND operation_id IS NOT NULL ORDER BY id")
                .unwrap()
                .query_map([page_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            compared_pages += 1;
            let page = run_page(
                &units,
                &words,
                &open_units,
                page_suggestions.contains(&page_id),
                number,
                &mut counters,
            );
            for (max, time) in source_times.iter_mut().zip(page.times) {
                *max = (*max).max(time);
            }
            orphans += page.orphans_a;
            orphan_pages += usize::from(page.orphans_a > 0);
            let by_id: HashMap<i64, &Unit> = units.iter().map(|u| (u.id, u)).collect();
            for (id, verdict) in &page.verdicts_a {
                let unit = by_id[id];
                bump(&mut a_verdicts, label(verdict));
                bump(&mut a_kinds, format!("{}|{}", unit.kind, label(verdict)));
                if let Verdict::Excluded(reason) = verdict {
                    bump(&mut reasons, format!("{reason:?}"));
                }
                if let Verdict::Differs { reason, .. } = verdict {
                    bump(&mut reasons, format!("DiffReason::{reason:?}"));
                }
            }
            for (id, verdict) in &page.verdicts_b {
                let unit = by_id[id];
                bump(&mut b_verdicts, label(verdict));
                bump(&mut b_kinds, format!("{}|{}", unit.kind, label(verdict)));
                bump(
                    safety.entry(unit.state.clone()).or_default(),
                    label(verdict),
                );
                let by_table = unit.state == "rejected"
                    && rejecting
                        .get(id)
                        .is_some_and(|operation| table_operations.contains(operation));
                rejected_by_table += usize::from(by_table);
                rejected_by_table_agrees += usize::from(by_table && *verdict == Verdict::Agrees);
                if *verdict != Verdict::Agrees {
                    continue;
                }
                if unit.state == "corrected" {
                    corrected_agrees.push(format!(
                        "unit {id} page {} whitespace_or_case_only={}",
                        page.number,
                        whitespace_or_case_only(&unit.raw_text, &unit.correction)
                    ));
                }
                let inside = table_words::words_among(&words, &[unit.polygon.as_str()]);
                if inside.is_empty() {
                    continue;
                }
                let heights: Vec<f32> = inside.iter().map(|w| w.bounds[3] - w.bounds[1]).collect();
                let cut = 0.7 * median(heights.clone());
                let small = heights.iter().filter(|h| **h < cut).count();
                agreeing_words += inside.len();
                raised_words += small;
                raised_units += usize::from(small > 0);
            }
        }
        println!(
            "  pages compared={compared_pages} no_word_row={no_row} invalid_row={invalid}; corrected units on pages without a usable word row={corrected_without_words}"
        );
        println!(
            "  max time per page ({}): propose={:?} shape scoring={:?} evaluate (a)={:?} evaluate (b)={:?} decode={:?}",
            if cfg!(debug_assertions) {
                "debug build"
            } else {
                "release build"
            },
            source_times[0],
            source_times[1],
            source_times[2],
            source_times[3],
            source_times[4]
        );
        print_counts("units by state/origin", &by_state);
        println!("  (a) as is");
        print_counts("verdicts", &a_verdicts);
        print_counts("exclusion and difference reasons", &reasons);
        print_counts("kind|verdict", &a_kinds);
        println!(
            "  orphan words={orphans} pages_with_orphans={orphan_pages}; totals before any exclusion order: unreviewed extracted units in a table proposal={}, units a shape alone scores >= {MIN_SCORE}={}",
            counters[0], counters[1]
        );
        println!(
            "  (b) as extracted (safety view; upper bound: no proposal, shape or suggestion exclusions)"
        );
        print_counts("verdicts", &b_verdicts);
        print_counts("kind|verdict", &b_kinds);
        for state in ["corrected", "rejected", "accepted", "unreviewed"] {
            print_counts(
                &format!("owner state {state}"),
                safety.get(state).unwrap_or(&Counts::new()),
            );
        }
        println!(
            "  rejected units rejected by an owner-table creation={rejected_by_table}, of which Agree in (b)={rejected_by_table_agrees}"
        );
        println!("  corrected units that Agree: {}", corrected_agrees.len());
        for line in &corrected_agrees {
            println!("    {line}");
        }
        println!(
            "  (b) Agrees units, separate small words only (< 0.7 x unit median height; cannot see a raised digit merged into one word): words={raised_words} of {agreeing_words}, units={raised_units}"
        );
        for (max, time) in max_times.iter_mut().zip(source_times) {
            *max = (*max).max(time);
        }
    }
    println!(
        "max times over all sources (propose, shape scoring, evaluate a, evaluate b, decode): {max_times:?}"
    );
}

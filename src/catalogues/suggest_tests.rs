//! The name cache and the matching of the suggest route (assisted sheet step 4, items 8, 9).
use super::{Entry, NameCache, NameIndex, Query, TYPO_BUDGET, suggest};
use crate::text_fold::name_norm;
use std::{sync::Arc, time::Duration};

fn entry(key: &str, kind: usize, name: &str, retired: bool) -> Entry {
    let norm = name_norm(name);
    Entry {
        key: key.into(),
        name: name.into(),
        scalars: norm.chars().count(),
        norm,
        kind,
        retired,
    }
}

fn index(entries: Vec<Entry>) -> NameIndex {
    NameIndex {
        types: vec![
            ("tool".into(), "Tool".into()),
            ("weapon".into(), "Weapon".into()),
        ],
        entries,
    }
}

/// The names suggested for `typed`, with the typo pass on or off.
fn names(index: &NameIndex, entry_type: Option<&str>, typed: &str, typo: bool) -> Vec<String> {
    let norm = name_norm(typed);
    let query = Query {
        scalars: norm.chars().count(),
        norm: &norm,
        typo,
    };
    suggest(index, entry_type, &query, &mut || Duration::ZERO)
        .items
        .iter()
        .map(|found| found.entry.key.clone())
        .collect()
}

#[test]
fn prefix_comes_before_substring_then_norm_name_key() {
    let index = index(vec![
        entry("b-lamp", 0, "Brass lamp", false),
        entry("lamp-b", 0, "Lamp", false),
        entry("lamp-a", 0, "lamp", false),
        entry("oil-lamp", 0, "Oil lamp", false),
        entry("lampion", 0, "Lampion", false),
    ]);
    // Prefix tier by (norm, name, key): "lamp" ("Lamp" < "lamp"), then "lampion"; then the
    // substring tier by norm.
    assert_eq!(
        names(&index, None, "la", false),
        ["lamp-b", "lamp-a", "lampion", "b-lamp", "oil-lamp"]
    );
    assert_eq!(
        names(&index, None, "lamp", false),
        ["lamp-b", "lamp-a", "lampion", "b-lamp", "oil-lamp"]
    );
    assert_eq!(names(&index, None, "amp", false).len(), 5, "substring only");
}

#[test]
fn only_eligible_entries_match_at_every_tier() {
    let index = index(vec![
        entry("rope", 0, "Rope", false),
        entry("old-rope", 0, "Old rope", true),
        entry("rope-w", 1, "Rope dart", false),
        entry("rape", 0, "Rape", false),
    ]);
    assert_eq!(names(&index, Some("tool"), "rop", false), ["rope"]);
    assert_eq!(names(&index, Some("tool"), "ope", false), ["rope"]);
    assert_eq!(names(&index, Some("tool"), "rpe", true), ["rape", "rope"]);
    assert_eq!(names(&index, Some("weapon"), "rop", false), ["rope-w"]);
    assert_eq!(
        names(&index, Some("nothing"), "rop", true),
        Vec::<String>::new()
    );
    assert_eq!(names(&index, None, "rop", false), ["rope", "rope-w"]);
}

#[test]
fn typo_distance_one_and_two_with_transposition_and_length_bounds() {
    let index = index(vec![
        entry("form", 0, "Form", false),
        entry("longsword", 0, "Longsword", false),
        entry("lantern", 0, "Lantern", false),
    ]);
    // A transposition is distance 1; short queries (3 to 5 scalars) allow only 1.
    assert_eq!(names(&index, None, "from", true), ["form"]);
    assert_eq!(names(&index, None, "fxxm", true), Vec::<String>::new());
    assert!(
        names(&index, None, "from", false).is_empty(),
        "no typo pass unless asked"
    );
    // From 6 scalars distance 2 passes, 3 does not; the length may differ by 2, not 3.
    assert_eq!(names(&index, None, "longswxrd", true), ["longsword"]);
    assert_eq!(names(&index, None, "lonnsxord", true), ["longsword"]);
    assert_eq!(names(&index, None, "lonnsxxrd", true), Vec::<String>::new());
    assert_eq!(names(&index, None, "longswo", true), ["longsword"]);
    assert_eq!(
        names(&index, None, "lonswrd", true),
        ["longsword"],
        "length differs by 2"
    );
    assert!(
        names(&index, None, "lonswd", true).is_empty(),
        "length differs by 3"
    );
    assert_eq!(names(&index, None, "lantren", true), ["lantern"]);
}

#[test]
fn folding_covers_diacritics_sharp_s_width_hangul_and_cjk_substrings() {
    let index = index(vec![
        entry("strasse", 0, "Stra\u{df}e", false),
        entry("epee", 0, "\u{c9}p\u{e9}e", false),
        entry("sword-w", 0, "\u{ff33}word", false),
        entry("hangul", 0, "\u{d55c}\u{ae00}\u{c774}\u{b984}", false),
        entry("cjk", 0, "\u{9577}\u{5263}\u{306e}\u{9375}", false),
    ]);
    assert_eq!(names(&index, None, "STRASSE", false), ["strasse"]);
    assert_eq!(names(&index, None, "stra\u{df}", false), ["strasse"]);
    assert_eq!(names(&index, None, "epee", false), ["epee"]);
    assert_eq!(names(&index, None, "\u{e9}pee", false), ["epee"]);
    assert_eq!(names(&index, None, "sword", false), ["sword-w"]);
    assert_eq!(names(&index, None, "\u{d55c}\u{ae00}", false), ["hangul"]);
    // A CJK substring (not a prefix) matches at the substring tier.
    assert_eq!(names(&index, None, "\u{5263}\u{306e}", false), ["cjk"]);
}

#[test]
fn duplicates_are_flagged_and_more_is_reported_at_nine_matches() {
    let mut entries = vec![
        entry("sword-a", 0, "Sword", false),
        entry("sword-b", 0, "SWORD", false),
        entry("shield", 0, "Shield", false),
    ];
    for n in 0..8 {
        entries.push(entry(&format!("s{n}"), 0, &format!("Sa{n}"), false));
    }
    let index = index(entries);
    let norm = name_norm("s");
    let query = Query {
        norm: &norm,
        scalars: 1,
        typo: false,
    };
    let found = suggest(&index, None, &query, &mut || Duration::ZERO);
    assert_eq!(found.items.len(), 8);
    assert!(found.more);
    assert!(found.items.iter().all(|found| !found.duplicate));
    let norm = name_norm("sw");
    let query = Query {
        norm: &norm,
        scalars: 2,
        typo: false,
    };
    let found = suggest(&index, None, &query, &mut || Duration::ZERO);
    assert!(!found.more);
    let swords: Vec<(&str, bool)> = found
        .items
        .iter()
        .map(|found| (found.entry.key.as_str(), found.duplicate))
        .collect();
    assert_eq!(swords, [("sword-b", true), ("sword-a", true)]);
    // A retired twin does not make an entry a duplicate.
    let index = super::tests::index(vec![
        entry("sword-a", 0, "Sword", false),
        entry("sword-b", 0, "Sword", true),
    ]);
    let norm = name_norm("sword");
    let query = Query {
        norm: &norm,
        scalars: 5,
        typo: false,
    };
    let found = suggest(&index, None, &query, &mut || Duration::ZERO);
    assert_eq!((found.items.len(), found.items[0].duplicate), (1, false));
}

#[test]
fn the_typo_pass_stops_at_its_budget_and_says_so() {
    let index = index(vec![
        entry("a", 0, "Swords", false),
        entry("b", 0, "Sworde", false),
        entry("c", 0, "Swordf", false),
    ]);
    let norm = name_norm("swordx");
    let query = Query {
        norm: &norm,
        scalars: 6,
        typo: true,
    };
    // A clock that advances 12 ms per reading: the budget is reached before the third
    // comparison, so two entries were compared and the answer is marked cut.
    let mut ticks = 0_u32;
    let mut clock = || {
        let now = Duration::from_millis(u64::from(ticks) * 12);
        ticks += 1;
        now
    };
    let found = suggest(&index, None, &query, &mut clock);
    assert!(found.cut);
    assert_eq!(found.items.len(), 2);
    assert_eq!(TYPO_BUDGET, Duration::from_millis(20));
    // Without the pass, nothing is cut.
    let found = suggest(
        &index,
        None,
        &Query {
            typo: false,
            ..query
        },
        &mut clock,
    );
    assert!(!found.cut);
}

fn sized(names: usize) -> Arc<NameIndex> {
    Arc::new(index(
        (0..names)
            .map(|n| entry(&format!("k{n}"), 0, &format!("Name {n}"), false))
            .collect(),
    ))
}

#[test]
fn the_cache_stays_within_its_budget_under_eviction_and_skips_an_oversized_index() {
    let one = sized(40).bytes();
    let cache = NameCache::new(one * 2 + one / 2);
    for version in 1..=5 {
        cache.insert((version, "h".into()), sized(40));
        assert!(cache.bytes() <= one * 2 + one / 2);
    }
    assert_eq!(cache.len(), 2);
    assert!(cache.get(&(5, "h".into())).is_some());
    assert!(cache.get(&(4, "h".into())).is_some());
    assert!(cache.get(&(1, "h".into())).is_none());
    // Reading marks an index recently used: 4 is touched, so 5 is evicted first.
    cache.get(&(4, "h".into()));
    cache.insert((6, "h".into()), sized(40));
    assert!(cache.get(&(4, "h".into())).is_some());
    assert!(cache.get(&(5, "h".into())).is_none());
    // The same key again replaces without double-counting.
    let before = cache.bytes();
    cache.insert((6, "h".into()), sized(40));
    assert_eq!(cache.bytes(), before);
    // An index above the whole budget is served by its caller and never kept.
    let big = sized(400);
    assert!(big.bytes() > one * 2 + one / 2);
    cache.insert((9, "h".into()), big);
    assert!(cache.get(&(9, "h".into())).is_none());
    assert!(cache.bytes() <= one * 2 + one / 2);
}

#[test]
fn a_new_seal_is_a_new_cache_key() {
    let cache = NameCache::new(1 << 20);
    cache.insert((1, "aa".into()), sized(3));
    assert!(cache.get(&(1, "bb".into())).is_none());
    assert!(cache.get(&(2, "aa".into())).is_none());
    assert!(cache.get(&(1, "aa".into())).is_some());
}

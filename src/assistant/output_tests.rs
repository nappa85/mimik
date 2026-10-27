use super::*;
use serde_json::json;

fn supplied(address: &str, integer: bool, value: Option<i64>) -> Supplied {
    Supplied {
        address: address.into(),
        integer,
        value,
    }
}

/// Target `hp` at 84; `f1` Max HP 84, `f2` an enum, `f3` an unknown integer, `c1` a
/// same-row cell at 3.
fn checks(phrase: &str) -> Checks {
    checks_at(phrase, Some(84))
}

fn checks_at(phrase: &str, target: Option<i64>) -> Checks {
    Checks {
        phrase: phrase.into(),
        target_value: target,
        supplied: BTreeMap::from([
            ("t".into(), supplied("field:hp", true, target)),
            ("f1".into(), supplied("field:max_hp", true, Some(84))),
            ("f2".into(), supplied("field:class", false, None)),
            ("f3".into(), supplied("field:luck", true, None)),
            (
                "c1".into(),
                supplied("cell:gear/r1-1/weight", true, Some(3)),
            ),
        ]),
    }
}

fn options(exprs: &[Value]) -> String {
    json!({"kind": "options", "options": exprs.iter().map(|e| json!({"expr": e, "explanation": "x"})).collect::<Vec<_>>()})
        .to_string()
}

fn one(expr: Value, phrase: &str) -> Result<i64, Drop> {
    one_with(expr, &checks(phrase))
}

fn one_with(expr: Value, checks: &Checks) -> Result<i64, Drop> {
    match read(&options(&[expr]), checks).expect("schema-valid") {
        Reading::Options { shown, dropped } => match (shown.as_slice(), dropped.as_slice()) {
            ([candidate], []) => Ok(candidate.value),
            ([], [reason]) => Err(*reason),
            other => panic!("{other:?}"),
        },
        Reading::Ambiguous(_) => panic!("ambiguous"),
    }
}

fn node(op: &str, round: Option<&str>, l: Value, r: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("op".into(), op.into());
    if let Some(round) = round {
        object.insert("round".into(), round.into());
    }
    object.insert("l".into(), l);
    object.insert("r".into(), r);
    Value::Object(object)
}

fn sub(l: Value, r: Value) -> Value {
    node("sub", None, l, r)
}

fn div(l: Value, r: Value, round: &str) -> Value {
    node("div", Some(round), l, r)
}

fn lit(n: i64) -> Value {
    json!({"lit": n})
}

fn alias(a: &str) -> Value {
    json!({"ref": a})
}

#[test]
fn invalid_replies_show_nothing() {
    let c = checks("took 15");
    for text in [
        "",
        "not json",
        r#"{"kind": "options", "options": [{"expr": {"lit": 15}"#,
        r#"{"kind": "options"}"#,
        r#"{"kind": "maybe", "options": []}"#,
        r#"{"kind": "options", "options": [], "extra": 1}"#,
        r#"{"kind": "options", "options": [{"expr": "t - 15", "explanation": "x"}]}"#,
        r#"{"kind": "options", "options": [{"expr": [1, 2], "explanation": "x"}]}"#,
        r#"{"kind": "options", "options": [{"expr": {"lit": 15}}]}"#,
    ] {
        assert!(read(text, &c).is_err(), "{text}");
    }
    let four = options(&[lit(15), lit(15), lit(15), lit(15)]);
    assert_eq!(read(&four, &c), Err(Invalid::Schema));
    let deep = json!({"op": "add", "l": {"op": "add", "l": {"op": "add", "l": {"op": "add",
        "l": lit(15), "r": lit(15)}, "r": lit(15)}, "r": lit(15)}, "r": lit(15)});
    assert_eq!(read(&options(&[deep]), &c), Err(Invalid::Schema));
}

#[test]
fn a_tree_of_depth_three_with_eight_leaves_is_read() {
    let add = |l: Value, r: Value| json!({"op": "add", "l": l, "r": r});
    let pair = || add(lit(1), lit(1));
    let tree = add(add(pair(), pair()), add(pair(), pair()));
    assert_eq!(one(tree, "1 more"), Ok(8));
}

#[test]
fn deeper_trees_are_dropped_even_past_the_schema() {
    let leaf = Node::Lit { lit: 1 };
    let mut node = leaf.clone();
    for _ in 0..4 {
        node = Node::Op {
            op: BinOp::Add,
            round: None,
            l: Box::new(node),
            r: Box::new(leaf.clone()),
        };
    }
    assert_eq!(depth(&node), 4);
}

#[test]
fn literal_provenance() {
    // `84 + 915`: 915 is not in the phrase.
    assert_eq!(
        one(json!({"op": "add", "l": lit(84), "r": lit(915)}), "heal 15"),
        Err(Drop::Provenance)
    );
    // `5` from "heal 15" is not a number of the phrase.
    assert_eq!(
        one(sub(alias("t"), lit(5)), "heal 15"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(
            json!({"op": "add", "l": alias("t"), "r": lit(15)}),
            "heal 15"
        ),
        Ok(99)
    );
    assert_eq!(
        one(sub(alias("t"), lit(15)), "take \u{2212}15 damage"),
        Ok(69)
    );
    assert_eq!(one(sub(alias("t"), lit(15)), "took\u{a0}15"), Ok(69));
    // A negative literal is checked by its magnitude; the target's value is admitted.
    assert_eq!(
        one(json!({"op": "add", "l": lit(84), "r": lit(-15)}), "lost 15"),
        Ok(69)
    );
    // Fraction words admit 2, 3, 4 and 100 only with a fraction word.
    assert_eq!(one(div(alias("t"), lit(2), "floor"), "half of it"), Ok(42));
    assert_eq!(one(div(alias("t"), lit(2), "floor"), "metà"), Ok(42));
    assert_eq!(one(div(alias("t"), lit(2), "floor"), "meta"), Ok(42));
    assert_eq!(
        one(div(alias("t"), lit(2), "floor"), "halfway"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(div(alias("t"), lit(4), "floor"), "quarterly"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(div(alias("t"), lit(2), "floor"), "lose some"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(div(alias("t"), lit(5), "floor"), "half"),
        Err(Drop::Provenance)
    );
}

fn mul(l: Value, r: Value) -> Value {
    node("mul", None, l, r)
}

#[test]
fn number_words_are_literals_with_provenance() {
    // ASSIST-53: "one" is 1, so `t - days * 1` has provenance (days is a cell at 3).
    assert_eq!(
        one(
            sub(alias("t"), mul(alias("c1"), lit(1))),
            "one lost for each day"
        ),
        Ok(81)
    );
    assert_eq!(one(sub(alias("t"), lit(10)), "ten"), Ok(74));
    assert_eq!(one(sub(alias("t"), lit(10)), "dieci"), Ok(74));
    assert_eq!(one(sub(alias("t"), lit(10)), "Ten damage"), Ok(74));
    assert_eq!(
        one(sub(alias("t"), lit(1)), "someone hit me"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(sub(alias("t"), lit(1)), "alone in the dark"),
        Err(Drop::Provenance)
    );
    assert_eq!(
        one(sub(alias("t"), lit(1)), "a point"),
        Err(Drop::Provenance)
    );
    // Only the words' own values: "ten" does not admit 11.
    assert_eq!(one(sub(alias("t"), lit(11)), "ten"), Err(Drop::Provenance));
}

#[test]
fn a_literal_one_as_multiplier_or_divisor_is_redundant() {
    let phrase = "lost them";
    assert_eq!(one(mul(alias("t"), lit(1)), phrase), Ok(84));
    assert_eq!(one(mul(lit(1), alias("t")), phrase), Ok(84));
    assert_eq!(one(div(alias("t"), lit(1), "exact"), phrase), Ok(84));
    // Nested and combined with a phrase number.
    assert_eq!(
        one(sub(alias("t"), mul(lit(15), lit(1))), "lost 15"),
        Ok(69)
    );
    // Anywhere else a 1 needs provenance.
    assert_eq!(
        one(json!({"op": "add", "l": alias("t"), "r": lit(1)}), phrase),
        Err(Drop::Provenance)
    );
    assert_eq!(one(sub(alias("t"), lit(1)), phrase), Err(Drop::Provenance));
    assert_eq!(
        one(div(lit(1), alias("t"), "floor"), phrase),
        Err(Drop::Provenance)
    );
    // Only 1: other multipliers and negative one are not redundant.
    assert_eq!(one(mul(alias("t"), lit(2)), phrase), Err(Drop::Provenance));
    assert_eq!(one(mul(alias("t"), lit(-1)), phrase), Err(Drop::Provenance));
    // A 1 below a multiplier operand is still a plain literal.
    assert_eq!(
        one(
            mul(alias("t"), json!({"op": "add", "l": lit(1), "r": lit(1)})),
            phrase
        ),
        Err(Drop::Provenance)
    );
}

#[test]
fn two_redundant_ones_do_not_manufacture_a_one() {
    // A 1 is exempt only beside an operand that is not a literal 1: `1 * 1` and `1 / 1`
    // are checked like any literals, and so is everything built from them.
    let phrase = "lost them";
    let one_one = || mul(lit(1), lit(1));
    let add = |l: Value, r: Value| json!({"op": "add", "l": l, "r": r});
    let refused = [
        one_one(),
        sub(alias("t"), one_one()),
        add(alias("t"), one_one()),
        sub(alias("t"), div(lit(1), lit(1), "exact")),
        add(alias("t"), add(one_one(), one_one())),
        mul(lit(15), add(one_one(), one_one())),
        mul(one_one(), lit(1)),
        div(one_one(), lit(1), "exact"),
        div(lit(1), lit(1), "exact"),
    ];
    for expr in refused {
        assert_eq!(one(expr.clone(), phrase), Err(Drop::Provenance), "{expr}");
    }
    // With a 1 in the note they are plain literals again.
    assert_eq!(one(sub(alias("t"), one_one()), "lost 1"), Ok(83));
    // Beside a reference, a subexpression or a provenanced literal the 1 stays exempt.
    assert_eq!(one(mul(alias("t"), lit(1)), phrase), Ok(84));
    assert_eq!(one(mul(sub(alias("t"), lit(84)), lit(1)), phrase), Ok(0));
    assert_eq!(
        one(sub(alias("t"), mul(lit(15), lit(1))), "lost 15"),
        Ok(69)
    );
    assert_eq!(
        one(
            sub(alias("t"), mul(alias("c1"), lit(1))),
            "one lost for each day"
        ),
        Ok(81)
    );
}

#[test]
fn references() {
    assert_eq!(one(sub(alias("f1"), lit(15)), "15 below max"), Ok(69));
    assert_eq!(
        one(
            json!({"op": "add", "l": alias("t"), "r": alias("c1")}),
            "add it"
        ),
        Ok(87)
    );
    assert_eq!(one(alias("f2"), "x"), Err(Drop::EnumRef));
    assert_eq!(one(alias("f3"), "x"), Err(Drop::UnknownRef));
    assert_eq!(one(alias("f9"), "x"), Err(Drop::UnsuppliedRef));
    assert_eq!(one(alias("c2"), "x"), Err(Drop::UnsuppliedRef));
    // An unknown target cannot be referenced, and its value admits no literal.
    let unknown = checks_at("heal 15", None);
    assert_eq!(
        one_with(
            json!({"op": "add", "l": alias("t"), "r": lit(15)}),
            &unknown
        ),
        Err(Drop::UnknownRef)
    );
    assert_eq!(one_with(lit(15), &unknown), Ok(15));
}

#[test]
fn used_references_are_recorded_once() {
    let text = options(&[
        json!({"op": "add", "l": alias("t"), "r": {"op": "sub", "l": alias("c1"), "r": alias("c1")}}),
    ]);
    let Ok(Reading::Options { shown, .. }) = read(&text, &checks("x")) else {
        panic!()
    };
    let refs: Vec<&str> = shown[0].refs.keys().map(String::as_str).collect();
    assert_eq!(refs, ["c1", "t"]);
    assert_eq!(shown[0].refs["c1"].address, "cell:gear/r1-1/weight");
}

#[test]
fn division_rounding_on_negatives() {
    let cases = [
        (7, 2, Round::Floor, Ok(3)),
        (7, 2, Round::Ceil, Ok(4)),
        (-7, 2, Round::Floor, Ok(-4)),
        (-7, 2, Round::Ceil, Ok(-3)),
        (7, -2, Round::Floor, Ok(-4)),
        (7, -2, Round::Ceil, Ok(-3)),
        (-7, -2, Round::Floor, Ok(3)),
        (-7, -2, Round::Ceil, Ok(4)),
        (8, 2, Round::Exact, Ok(4)),
        (-8, 2, Round::Exact, Ok(-4)),
        (7, 2, Round::Exact, Err(Drop::Inexact)),
        (-7, 2, Round::Exact, Err(Drop::Inexact)),
        (7, 0, Round::Floor, Err(Drop::DivisionByZero)),
        (i128::MIN, -1, Round::Floor, Err(Drop::Overflow)),
    ];
    for (l, r, round, expected) in cases {
        assert_eq!(divide(l, r, round), expected, "{l} / {r} {round:?}");
    }
    assert_eq!(
        one(div(alias("t"), lit(0), "floor"), "0"),
        Err(Drop::DivisionByZero)
    );
    assert_eq!(one(div(lit(-7), lit(2), "floor"), "-7 half"), Ok(-4));
    assert_eq!(
        one(div(lit(7), lit(2), "exact"), "7 half"),
        Err(Drop::Inexact)
    );
}

#[test]
fn i64_boundaries_and_intermediate_overflow() {
    let max = i64::MAX;
    let at_max = checks_at(&format!("{max}"), Some(max));
    // 18 digits at most in the phrase, but the target's value is admitted whatever its size.
    assert_eq!(one_with(alias("t"), &at_max), Ok(max));
    assert_eq!(
        one_with(
            json!({"op": "add", "l": alias("t"), "r": alias("t")}),
            &at_max
        ),
        Err(Drop::Overflow)
    );
    // An intermediate value past i64 that comes back into range is fine (checked i128).
    assert_eq!(
        one_with(
            sub(
                json!({"op": "add", "l": alias("t"), "r": alias("t")}),
                alias("t")
            ),
            &at_max
        ),
        Ok(max)
    );
    // Past i128: t × t × t × t with t = i64::MAX.
    let mul = |l: Value, r: Value| json!({"op": "mul", "l": l, "r": r});
    let product = mul(mul(alias("t"), alias("t")), mul(alias("t"), alias("t")));
    assert_eq!(one_with(product, &at_max), Err(Drop::Overflow));
    let min = checks_at("x", Some(i64::MIN));
    assert_eq!(one_with(alias("t"), &min), Ok(i64::MIN));
    assert_eq!(
        one_with(
            json!({"op": "mul", "l": alias("t"), "r": {"lit": -1}}),
            &min
        ),
        Err(Drop::Provenance)
    );
    // A literal beyond i64 is a shape error of that option only.
    let text = r#"{"kind": "options", "options": [{"expr": {"lit": 10000000000000000000}, "explanation": "x"}, {"expr": {"lit": 15}, "explanation": "y"}]}"#;
    let Ok(Reading::Options { shown, dropped }) = read(text, &checks("heal 15")) else {
        panic!()
    };
    assert_eq!(shown.len(), 1);
    assert_eq!(dropped, [Drop::Shape]);
}

#[test]
fn ambiguous_readings_are_text_only() {
    let text =
        json!({"kind": "ambiguous", "readings": ["<b>lost</b> 15?", "gained 15?"]}).to_string();
    assert_eq!(
        read(&text, &checks("15")),
        Ok(Reading::Ambiguous(vec![
            "<b>lost</b> 15?".into(),
            "gained 15?".into()
        ]))
    );
    let four = json!({"kind": "ambiguous", "readings": ["a", "b", "c", "d"]}).to_string();
    assert_eq!(read(&four, &checks("15")), Err(Invalid::Schema));
    let long = json!({"kind": "ambiguous", "readings": ["é".repeat(121)]}).to_string();
    assert_eq!(read(&long, &checks("15")), Err(Invalid::Schema));
    let fits = json!({"kind": "ambiguous", "readings": ["é".repeat(120)]}).to_string();
    assert!(read(&fits, &checks("15")).is_ok());
}

#[test]
fn rendering_uses_display_symbols() {
    let node: Node = serde_json::from_value(json!({"op": "sub", "l": {"ref": "t"}, "r": {"op": "div",
        "round": "floor", "l": {"op": "mul", "l": {"lit": -15}, "r": {"ref": "f1"}}, "r": {"lit": 2}}}))
    .unwrap();
    let name = |alias: &str| {
        if alias == "t" {
            "84".to_owned()
        } else {
            "Max HP".to_owned()
        }
    };
    assert_eq!(
        render(&node, &name),
        "84 \u{2212} ((\u{2212}15 \u{d7} Max HP) \u{f7} 2 (rounded down))"
    );
}

/// Target `hp` at 84 and context items named by their IDs, among them `f1`, `c1`, `t1` and
/// `ta`, which are ordinary IDs (ASSIST-56).
fn named(phrase: &str) -> Checks {
    Checks {
        phrase: phrase.into(),
        target_value: Some(84),
        supplied: BTreeMap::from([
            ("t".into(), supplied("field:hp", true, Some(84))),
            ("max_hp".into(), supplied("field:max_hp", true, Some(90))),
            ("class".into(), supplied("field:class", false, None)),
            ("luck".into(), supplied("field:luck", true, None)),
            ("f1".into(), supplied("field:f1", true, Some(7))),
            ("c1".into(), supplied("cell:gear/r1-1/c1", true, Some(3))),
            ("t1".into(), supplied("field:t1", true, Some(11))),
            ("ta".into(), supplied("field:ta", true, Some(13))),
        ]),
    }
}

#[test]
fn a_reference_by_id_is_evaluated_with_the_value_bound_for_it() {
    let c = named("lost 15");
    assert_eq!(one_with(sub(alias("max_hp"), lit(15)), &c), Ok(75));
    for (id, value) in [("f1", 7), ("c1", 3), ("t1", 11), ("ta", 13)] {
        let reading = read(&options(&[sub(alias("t"), alias(id))]), &c).unwrap();
        let Reading::Options { shown, dropped } = reading else {
            panic!("options")
        };
        assert!(dropped.is_empty(), "{id}");
        assert_eq!(shown[0].value, 84 - value, "{id}");
        // The reference is kept by its ID with the value it was evaluated with.
        assert_eq!(shown[0].refs[id].value, Some(value));
    }
}

#[test]
fn an_unsupplied_id_drops_only_its_option_not_the_reply() {
    let c = named("lost 15");
    // `weight` was never supplied (or was dropped by the budget or as ambiguous), `luck` is
    // unknown, `class` is an enum, `f2` is not an ID of this request.
    let first = options(&[
        sub(alias("t"), alias("weight")),
        sub(alias("t"), lit(15)),
        sub(alias("t"), alias("luck")),
    ]);
    let Reading::Options { shown, dropped } = read(&first, &c).unwrap() else {
        panic!("options")
    };
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].value, 69);
    assert_eq!(dropped, [Drop::UnsuppliedRef, Drop::UnknownRef]);
    let second = options(&[
        sub(alias("t"), alias("class")),
        sub(alias("t"), alias("f2")),
    ]);
    let Reading::Options { shown, dropped } = read(&second, &c).unwrap() else {
        panic!("options")
    };
    assert_eq!(shown, Vec::new());
    assert_eq!(dropped, [Drop::EnumRef, Drop::UnsuppliedRef]);
}

/// ASSIST-L52: a context value the model guesses is checked like any literal, so a guess that
/// equals a number of the note is admitted. The model is not shown the values, which makes
/// such a guess unlikely but not impossible.
#[test]
fn a_guessed_context_value_equal_to_a_note_number_has_provenance() {
    let c = named("heal 90");
    // `max_hp` is 90 and the note says 90: `t + 90` has provenance by the note.
    assert_eq!(
        one_with(json!({"op": "add", "l": alias("t"), "r": lit(90)}), &c),
        Ok(174)
    );
    // A guess that is not a number of the note is refused.
    assert_eq!(
        one_with(json!({"op": "add", "l": alias("t"), "r": lit(91)}), &c),
        Err(Drop::Provenance)
    );
}

/// ASSIST-56 puts IDs, up to 64 characters, in replies. The largest reply the schema admits
/// (3 options of 8 leaves, 120-character explanations) with 64-character IDs is far longer
/// than the default `max_tokens` allows, so such a reply is cut off; a cut-off reply is not
/// JSON and shows nothing (documented in ASSIST-L52). Models answer with short expressions in
/// practice (the experiments' replies are well under the cap); this pins the safe failure.
#[test]
fn a_reply_cut_off_at_max_tokens_shows_nothing_and_a_complete_one_is_read() {
    let id = "a".repeat(64);
    let mut supplied = BTreeMap::from([("t".into(), supplied("field:hp", true, Some(84)))]);
    supplied.insert(id.clone(), self::supplied("field:a", true, Some(1)));
    let c = Checks {
        phrase: "lost 1".into(),
        target_value: Some(84),
        supplied,
    };
    let add = |l: Value, r: Value| json!({"op": "add", "l": l, "r": r});
    let pair = || add(alias(&id), alias(&id));
    let tree = add(add(pair(), pair()), add(pair(), pair()));
    let option = json!({"expr": tree, "explanation": "x".repeat(120)});
    let reply =
        json!({"kind": "options", "options": [option.clone(), option.clone(), option]}).to_string();
    // The most the schema admits is read whole...
    let Ok(Reading::Options { shown, .. }) = read(&reply, &c) else {
        panic!("the largest reply is valid")
    };
    assert_eq!(shown.len(), 3);
    // ...is more than 352 tokens even at 4 bytes per token (about 2.8 KiB)...
    assert!(reply.len() > 352 * 4, "{}", reply.len());
    // ...and no cut of it, as the cap makes, is read as anything.
    for end in (1..reply.len()).step_by(37) {
        assert!(read(&reply[..end], &c).is_err(), "{end}");
    }
}

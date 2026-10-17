//! Pure, explicitly limited base-rule checklists. These do not establish full legality.
use crate::{AppError, Character, Store, mork_borg::Sheet};
use serde::Deserialize;

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Truth {
    #[default]
    Unknown,
    Yes,
    No,
}

impl Truth {
    #[must_use]
    pub fn value(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub revision: i64,
    #[serde(default)]
    pub base: Truth,
    #[serde(default)]
    pub can_act: Truth,
    #[serde(default)]
    pub action_remaining: Truth,
    #[serde(default)]
    pub movement_remaining: Truth,
    #[serde(default)]
    pub melee_reach: Truth,
    #[serde(default)]
    pub ranged_ready: Truth,
    #[serde(default)]
    pub ranged_in_range: Truth,
    #[serde(default)]
    pub incoming_attack: Truth,
    #[serde(default)]
    pub scroll_available: Truth,
    #[serde(default)]
    pub blocking_armor: Truth,
    #[serde(default)]
    pub zweihander: Truth,
    #[serde(default)]
    pub disoriented: Truth,
    #[serde(default)]
    pub route_clear: Truth,
    #[serde(default)]
    pub power_uses: String,
    #[serde(default, deserialize_with = "crate::form_text::lf")]
    pub goal: String,
}

pub struct FactField {
    pub name: &'static str,
    pub label: &'static str,
    pub value: &'static str,
}

impl Context {
    #[must_use]
    pub fn fields(&self) -> Vec<FactField> {
        [
            (
                "base",
                "Unmodified classless base rules, no special effects/house rules",
                self.base,
            ),
            (
                "can_act",
                "Can perform an ordinary turn action (separate from budget)",
                self.can_act,
            ),
            (
                "action_remaining",
                "Ordinary action remains this round",
                self.action_remaining,
            ),
            (
                "movement_remaining",
                "Ordinary room movement remains this round",
                self.movement_remaining,
            ),
            (
                "melee_reach",
                "Target is within melee reach",
                self.melee_reach,
            ),
            (
                "ranged_ready",
                "Usable loaded ranged weapon is ready",
                self.ranged_ready,
            ),
            (
                "ranged_in_range",
                "Target is within that weapon's range",
                self.ranged_in_range,
            ),
            (
                "incoming_attack",
                "An incoming attack calls for a defense roll",
                self.incoming_attack,
            ),
            (
                "scroll_available",
                "An applicable scroll is available",
                self.scroll_available,
            ),
            (
                "blocking_armor",
                "Actually wearing medium/heavy armor now",
                self.blocking_armor,
            ),
            (
                "zweihander",
                "Currently wielding a zweihander",
                self.zweihander,
            ),
            (
                "disoriented",
                "Currently in the failed-power disorientation hour",
                self.disoriented,
            ),
            (
                "route_clear",
                "Room traversal is possible in the situation",
                self.route_clear,
            ),
        ]
        .into_iter()
        .map(|(name, label, value)| FactField {
            name,
            label,
            value: value.value(),
        })
        .collect()
    }

    fn uses(&self) -> Result<Option<u16>, AppError> {
        if self.goal.chars().count() > 1_000 || self.power_uses.len() > 4 {
            return Err(AppError::Invalid(
                "Context goal is limited to 1,000 characters; known daily uses must be 0–1,000.",
            ));
        }
        if self.power_uses.trim().is_empty() {
            return Ok(None);
        }
        let uses = self.power_uses.parse::<u16>().map_err(|_| {
            AppError::Invalid(
                "Known daily power uses must be a nonnegative integer, or blank for unknown.",
            )
        })?;
        if uses > 1_000 {
            return Err(AppError::Invalid(
                "Known daily power uses exceed the 1,000 technical limit.",
            ));
        }
        Ok(Some(uses))
    }
}

pub struct Hint {
    pub key: &'static str,
    pub title: &'static str,
    pub status: &'static str,
    pub target: Option<i32>,
    pub modifier: Option<i32>,
    pub ability: &'static str,
    pub established: Vec<String>,
    pub blockers: Vec<String>,
    pub unknowns: Vec<String>,
    pub explanation: &'static str,
    pub citation: &'static str,
}

impl Hint {
    fn new(
        key: &'static str,
        title: &'static str,
        ability: &'static str,
        explanation: &'static str,
        citation: &'static str,
    ) -> Self {
        Self {
            key,
            title,
            ability,
            explanation,
            citation,
            status: "needs context",
            target: None,
            modifier: None,
            established: Vec::new(),
            blockers: Vec::new(),
            unknowns: Vec::new(),
        }
    }
    fn require(&mut self, fact: Truth, expected: Truth, label: &str) {
        match fact {
            Truth::Unknown => self.unknowns.push(format!("Confirm: {label}")),
            value if value == expected => self.established.push(label.into()),
            _ => self
                .blockers
                .push(format!("Reported condition does not support: {label}")),
        }
    }
    fn finish(&mut self, target: Option<i32>, modifier: Option<i32>) {
        self.status = if !self.blockers.is_empty() {
            "blocked standard path"
        } else if !self.unknowns.is_empty() {
            "needs context"
        } else {
            "standard check supported"
        };
        if self.status == "standard check supported" {
            self.target = target;
            self.modifier = modifier;
        }
    }
}

pub struct Advice {
    pub revision: i64,
    pub goal: String,
    pub notes: Vec<&'static str>,
    pub hints: Vec<Hint>,
}

fn cards() -> Vec<Hint> {
    vec![
        Hint::new(
            "melee",
            "Ordinary melee attack",
            "Strength",
            "Base melee is a Strength test. Weapon damage, special targets and exceptional effects are outside this checklist.",
            "IT2025 p.29 / PDF33; p.27 / PDF31 load; p.26 / PDF30 modifiers; action budget PDF34 (round text).",
        ),
        Hint::new(
            "ranged",
            "Ordinary ranged attack",
            "Presence",
            "Base ranged attacks use Presence. Weapon usability, ammunition and range are player-confirmed context, not inferred from notes.",
            "IT2025 p.29 / PDF33; action budget PDF34.",
        ),
        Hint::new(
            "power",
            "Normal scroll/power attempt",
            "Presence",
            "Normal use: Presence DR12. Success activates the power and spends one daily use. Failure loses d2 HP and causes one hour of disorientation; it is not described here as spending a successful daily use. Further attempts in that hour fail catastrophically. Critical/fumble effects require GM adjudication. No rolls or resources are changed automatically.",
            "IT2025 p.24 / PDF28 (armor/zweihander); p.34 / PDF38 (powers).",
        ),
        Hint::new(
            "move",
            "Ordinary room movement",
            "No fixed test specified",
            "A round permits an action and crossing a medium-sized room; movement is tracked separately. This is not a precise distance, speed or terrain permission.",
            "IT2025 PDF34 (round text; printed number not visible in source image).",
        ),
        Hint::new(
            "defense",
            "Triggered defense reference",
            "Agility",
            "Defense is a response to an incoming attack, independent of your ordinary action budget. Natural-20 defense grants an immediate free attack: that exception is outside this ordinary-action checklist and is not spent automatically.",
            "IT2025 p.29 / PDF33; p.24 / PDF28 armor; p.27 / PDF31 load.",
        ),
    ]
}

fn global_gate(sheet: &Sheet, saved: bool, context: &Context) -> Option<&'static str> {
    if !saved {
        return Some(
            "Save and confirm the actual character sheet first; defaults are not evidence for advice.",
        );
    }
    if context.base != Truth::Yes {
        return Some(
            "Confirm unmodified classless base scope first. Class features, special effects and house rules are not evaluated.",
        );
    }
    if sheet.hp == 0 {
        return Some(
            "Zero HP requires resolving the broken outcome and injury consequences. This is not a blanket cannot-act rule; targets are withheld pending adjudication.",
        );
    }
    if sheet.over_maximum() {
        return Some(
            "Resolve above-maximum carrying load with the GM before relying on these targets.",
        );
    }
    if (context.blocking_armor == Truth::Yes && sheet.worn_armor < 2)
        || (context.blocking_armor == Truth::No && sheet.worn_armor >= 2)
    {
        return Some(
            "Reported actual armor conflicts with the saved original worn tier. Update/confirm the sheet; damaged protection does not remove original penalties.",
        );
    }
    None
}

fn power(hint: &mut Hint, context: &Context, uses: Option<u16>) {
    ordinary_action(hint, context);
    hint.require(
        context.scroll_available,
        Truth::Yes,
        "applicable scroll available",
    );
    hint.require(
        context.blocking_armor,
        Truth::No,
        "not wearing medium/heavy armor",
    );
    hint.require(context.zweihander, Truth::No, "not wielding a zweihander");
    match uses {
        Some(0) => hint
            .blockers
            .push("No known daily power uses remain.".into()),
        Some(count) => hint
            .established
            .push(format!("Known remaining daily uses: {count}")),
        None => hint
            .unknowns
            .push("Confirm remaining daily power uses.".into()),
    }
    match context.disoriented {
        Truth::Yes => hint.unknowns.push("During this hour further attempts fail catastrophically under the reviewed base rule; GM adjudication required, no normal success path endorsed.".into()),
        Truth::Unknown => hint.unknowns.push("Confirm whether the failed-power disorientation hour applies.".into()),
        Truth::No => hint.established.push("No failed-power disorientation hour reported.".into()),
    }
    hint.finish(Some(12), None);
}

fn ordinary_action(hint: &mut Hint, context: &Context) {
    hint.require(
        context.can_act,
        Truth::Yes,
        "can perform an ordinary action",
    );
    hint.require(
        context.action_remaining,
        Truth::Yes,
        "ordinary action remains",
    );
}

fn evaluate_card(
    hint: &mut Hint,
    sheet: &Sheet,
    context: &Context,
    uses: Option<u16>,
) -> Result<(), AppError> {
    match hint.key {
        "melee" => {
            ordinary_action(hint, context);
            hint.require(
                context.melee_reach,
                Truth::Yes,
                "target is within melee reach",
            );
            hint.finish(Some(sheet.melee_dr()), Some(sheet.strength));
        }
        "ranged" => {
            ordinary_action(hint, context);
            hint.require(
                context.ranged_ready,
                Truth::Yes,
                "usable loaded ranged weapon ready",
            );
            hint.require(
                context.ranged_in_range,
                Truth::Yes,
                "target within confirmed weapon range",
            );
            hint.finish(Some(sheet.ranged_dr()), Some(sheet.presence));
        }
        "power" => {
            power(hint, context, uses);
            if hint.target.is_some() {
                hint.modifier = Some(sheet.presence);
            }
        }
        "move" => {
            hint.require(
                context.can_act,
                Truth::Yes,
                "ordinary movement physically possible",
            );
            hint.require(
                context.movement_remaining,
                Truth::Yes,
                "room movement remains",
            );
            hint.require(
                context.route_clear,
                Truth::Yes,
                "traversal possible in this situation",
            );
            hint.finish(None, None);
        }
        "defense" => {
            hint.require(
                context.incoming_attack,
                Truth::Yes,
                "incoming attack calls for defense",
            );
            if context.can_act != Truth::Yes {
                hint.unknowns
                    .push("Confirm incapacity/injury effects on defense with the GM.".into());
            }
            if context.blocking_armor == Truth::Unknown {
                hint.unknowns
                    .push("Confirm actual armor matches the saved sheet.".into());
            }
            hint.finish(Some(sheet.defense_dr()), Some(sheet.agility));
        }
        _ => return Err(AppError::Invalid("Unknown compiled checklist card.")),
    }
    Ok(())
}

/// Evaluate a bounded checklist without changing any canonical state.
///
/// # Errors
/// Rejects invalid context counts/goals; missing evidence yields explicit context states.
pub fn evaluate(sheet: &Sheet, saved: bool, context: &Context) -> Result<Advice, AppError> {
    let uses = context.uses()?;
    if saved {
        sheet.validate()?;
    }
    let mut hints = cards();
    let mut notes = vec![
        "These statuses describe only this bounded base-rule checklist, not full action legality. Facts are player declarations; targets are a saved-sheet snapshot.",
    ];
    if let Some(note) = global_gate(sheet, saved, context) {
        notes.push(note);
        for hint in &mut hints {
            hint.unknowns.push(note.into());
        }
    } else if sheet.hp < 0 {
        notes.push("Negative HP means dead under confirmed unmodified base rules. Ordinary actions are blocked; special survival effects/reactions/movement require adjudication outside scope.");
        for hint in &mut hints {
            if ["melee", "ranged", "power"].contains(&hint.key) {
                hint.status = "blocked standard path";
                hint.blockers.push("Dead under base rules.".into());
            } else {
                hint.unknowns
                    .push("Death/special-effect adjudication required.".into());
            }
        }
    } else {
        for hint in &mut hints {
            evaluate_card(hint, sheet, context, uses)?;
        }
    }
    Ok(Advice {
        revision: sheet.revision,
        goal: context.goal.clone(),
        notes,
        hints,
    })
}

impl Store {
    /// Build advice from one coherent persisted identity/sheet snapshot.
    ///
    /// # Errors
    /// Fails on wrong edition, stale expected revision, invalid context or storage errors.
    pub async fn mork_advice(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<(Character, Advice), AppError> {
        let (character, sheet, saved) = self.mork_borg_sheet(id).await?;
        if context.revision != sheet.revision {
            return Err(AppError::Conflict);
        }
        Ok((character, evaluate(&sheet, saved, context)?))
    }
}

//! First, explicitly limited adapter for the reviewed Italian core rules.
use crate::{AppError, Character, Store, store::load};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const EDITION: &str = "MÖRK BORG — IT 2025";

#[derive(Clone, Deserialize, Serialize)]
pub struct Sheet {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub class_notes: String,
    #[serde(default)]
    pub powers: String,
    #[serde(default)]
    pub weapon_one: String,
    #[serde(default)]
    pub weapon_two: String,
    #[serde(default)]
    pub campaign_miseries: Option<u8>,
    pub agility: i32,
    pub presence: i32,
    pub strength: i32,
    pub toughness: i32,
    pub hp: i32,
    pub max_hp: i32,
    /// Original worn tier: penalties remain when armor is damaged.
    pub worn_armor: i32,
    pub protection: i32,
    pub standard_items: i32,
    pub silver: i32,
    pub omens: i32,
    pub equipment: String,
    /// Shared character revision protects identity and sheet edits alike.
    pub revision: i64,
}

impl Default for Sheet {
    fn default() -> Self {
        Self {
            description: String::new(),
            class_notes: String::new(),
            powers: String::new(),
            weapon_one: String::new(),
            weapon_two: String::new(),
            campaign_miseries: None,
            agility: 0,
            presence: 0,
            strength: 0,
            toughness: 0,
            hp: 1,
            max_hp: 1,
            worn_armor: 0,
            protection: 0,
            standard_items: 0,
            silver: 0,
            omens: 0,
            equipment: String::new(),
            revision: 0,
        }
    }
}

impl Sheet {
    /// Check base-rule ranges and storage bounds without changing the sheet.
    ///
    /// # Errors
    /// Returns an input error for unsupported modifier, HP, armor or resource values.
    pub fn validate(&self) -> Result<(), AppError> {
        if [&self.description, &self.class_notes, &self.powers]
            .iter()
            .any(|value| value.chars().count() > 20_000)
            || [&self.weapon_one, &self.weapon_two]
                .iter()
                .any(|value| value.chars().count() > 2_000)
            || self.campaign_miseries.is_some_and(|value| value > 7)
        {
            return Err(AppError::Invalid(
                "Description/class/powers are limited to 20,000 characters, weapons to 2,000; campaign miseries reference must be 0–7 or unknown.",
            ));
        }
        if [self.agility, self.presence, self.strength, self.toughness]
            .iter()
            .any(|v| !(-3..=6).contains(v))
        {
            return Err(AppError::Invalid(
                "Base ability modifiers must be between −3 and +6. House-rule overrides are not implemented yet.",
            ));
        }
        if !(1..=100_000).contains(&self.max_hp) || !(-100_000..=self.max_hp).contains(&self.hp) {
            return Err(AppError::Invalid(
                "Maximum HP must be positive; current HP may be zero/negative but cannot exceed maximum HP. Storage limit: 100,000.",
            ));
        }
        if !(0..=3).contains(&self.worn_armor) || !(0..=self.worn_armor).contains(&self.protection)
        {
            return Err(AppError::Invalid(
                "Armor tiers must be 0–3; current protection cannot exceed the original worn tier.",
            ));
        }
        if [self.standard_items, self.silver, self.omens]
            .iter()
            .any(|v| !(0..=1_000_000).contains(v))
            || self.equipment.chars().count() > 20_000
        {
            return Err(AppError::Invalid(
                "Resources/item counts must be nonnegative (storage limit: 1,000,000); equipment notes are limited to 20,000 characters.",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn carrying_limit(&self) -> i32 {
        self.strength + 8
    }
    #[must_use]
    pub fn maximum_load(&self) -> i32 {
        2 * self.carrying_limit()
    }
    #[must_use]
    pub fn overburdened(&self) -> bool {
        self.standard_items > self.carrying_limit()
    }
    #[must_use]
    pub fn over_maximum(&self) -> bool {
        self.standard_items > self.maximum_load()
    }
    fn load_penalty(&self) -> i32 {
        if self.overburdened() { 2 } else { 0 }
    }
    #[must_use]
    pub fn melee_dr(&self) -> i32 {
        12 + self.load_penalty()
    }
    #[must_use]
    pub fn ranged_dr(&self) -> i32 {
        12
    }
    #[must_use]
    pub fn defense_dr(&self) -> i32 {
        12 + self.load_penalty() + if self.worn_armor >= 2 { 2 } else { 0 }
    }
    #[must_use]
    pub fn agility_dr(&self) -> i32 {
        12 + self.load_penalty()
            + match self.worn_armor {
                2 => 2,
                3 => 4,
                _ => 0,
            }
    }
    #[must_use]
    pub fn armor_reduction(&self) -> &'static str {
        match self.protection {
            1 => "d2",
            2 => "d4",
            3 => "d6",
            _ => "0",
        }
    }
    #[must_use]
    pub fn health_status(&self) -> &'static str {
        match self.hp.cmp(&0) {
            std::cmp::Ordering::Less => "Dead under the base rules",
            std::cmp::Ordering::Equal => "Broken: resolve the broken table",
            std::cmp::Ordering::Greater => "Positive HP",
        }
    }
}

impl Store {
    /// Load stats and identify whether the sheet has been saved.
    ///
    /// # Errors
    /// Fails for an unknown character, unsupported edition, corrupt data or storage failure.
    pub async fn mork_borg_sheet(&self, id: i64) -> Result<(Character, Sheet, bool), AppError> {
        self.run(move |db| {
            let tx = db.transaction()?;
            let character = load(&tx, id)?;
            require_adapter(&character)?;
            let data: Option<String> = tx
                .query_row(
                    "SELECT data FROM mork_borg_sheets WHERE character_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            let saved = data.is_some();
            let mut sheet: Sheet = match data {
                Some(data) => serde_json::from_str(&data)?,
                None => Sheet::default(),
            };
            sheet.revision = character.input.revision;
            tx.commit()?;
            Ok((character, sheet, saved))
        })
        .await
    }

    /// Save validated stats and increment the shared character revision atomically.
    ///
    /// # Errors
    /// Fails for invalid values, unsupported editions, stale revisions or storage failures.
    pub async fn save_mork_borg_sheet(
        &self,
        id: i64,
        sheet: Sheet,
    ) -> Result<(Character, Sheet), AppError> {
        self.save_mork_borg_sheet_named(id, sheet, None).await
    }

    /// Atomically update canonical identity name and stats using one shared revision.
    ///
    /// # Errors
    /// Rejects invalid name/stats, unsupported editions, stale revisions or storage failures.
    pub async fn save_mork_borg_sheet_named(
        &self,
        id: i64,
        mut sheet: Sheet,
        name: Option<String>,
    ) -> Result<(Character, Sheet), AppError> {
        sheet.validate()?;
        self.run(move |db| {
            let tx = db.transaction()?;
            let character = load(&tx, id)?;
            require_adapter(&character)?;
            if character.input.revision != sheet.revision { return Err(AppError::Conflict); }
            let mut identity = character.input.clone();
            if let Some(name) = name { identity.name = name; }
            identity = identity.normalize();
            identity.validate()?;
            tx.execute("UPDATE characters SET name=?1,revision = revision + 1 WHERE id = ?2 AND revision = ?3", params![identity.name,id, sheet.revision])?;
            sheet.revision += 1;
            let data = serde_json::to_string(&sheet)?;
            tx.execute("INSERT INTO mork_borg_sheets (character_id, data) VALUES (?1, ?2) ON CONFLICT(character_id) DO UPDATE SET data = excluded.data", params![id, data])?;
            let character = load(&tx, id)?;
            tx.commit()?;
            Ok((character, sheet))
        }).await
    }
}

fn require_adapter(character: &Character) -> Result<(), AppError> {
    if character.has_mork_borg_adapter() {
        Ok(())
    } else {
        Err(AppError::Invalid(
            "This adapter requires a MÖRK BORG — IT 2025 character. Other edition records remain separate.",
        ))
    }
}

//! Raw transport values are retained until validated; omissions preserve new fields.
use crate::{AppError, Character, mork_borg::Sheet};
use serde::Deserialize;

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SheetForm {
    pub name: Option<String>,
    pub revision: String,
    pub agility: String,
    pub presence: String,
    pub strength: String,
    pub toughness: String,
    pub hp: String,
    pub max_hp: String,
    pub worn_armor: String,
    pub protection: String,
    pub standard_items: String,
    pub silver: String,
    pub omens: String,
    pub equipment: String,
    pub description: Option<String>,
    pub class_notes: Option<String>,
    pub powers: Option<String>,
    pub weapon_one: Option<String>,
    pub weapon_two: Option<String>,
    pub campaign_miseries: Option<String>,
}

pub(crate) struct SheetDraft {
    pub name: String,
    pub revision: String,
    pub agility: String,
    pub presence: String,
    pub strength: String,
    pub toughness: String,
    pub hp: String,
    pub max_hp: String,
    pub worn_armor: String,
    pub protection: String,
    pub standard_items: String,
    pub silver: String,
    pub omens: String,
    pub equipment: String,
    pub description: String,
    pub class_notes: String,
    pub powers: String,
    pub weapon_one: String,
    pub weapon_two: String,
    pub campaign_miseries: String,
}

fn integer<T: std::str::FromStr>(value: &str, field: &'static str) -> Result<T, AppError> {
    value.parse().map_err(|_| AppError::Invalid(field))
}

impl SheetDraft {
    pub fn saved(character: &Character, sheet: &Sheet) -> Self {
        Self::from_sheet(&character.input.name, sheet)
    }

    pub(crate) fn from_sheet(name: &str, sheet: &Sheet) -> Self {
        Self {
            name: name.into(),
            revision: sheet.revision.to_string(),
            agility: sheet.agility.to_string(),
            presence: sheet.presence.to_string(),
            strength: sheet.strength.to_string(),
            toughness: sheet.toughness.to_string(),
            hp: sheet.hp.to_string(),
            max_hp: sheet.max_hp.to_string(),
            worn_armor: sheet.worn_armor.to_string(),
            protection: sheet.protection.to_string(),
            standard_items: sheet.standard_items.to_string(),
            silver: sheet.silver.to_string(),
            omens: sheet.omens.to_string(),
            equipment: sheet.equipment.clone(),
            description: sheet.description.clone(),
            class_notes: sheet.class_notes.clone(),
            powers: sheet.powers.clone(),
            weapon_one: sheet.weapon_one.clone(),
            weapon_two: sheet.weapon_two.clone(),
            campaign_miseries: sheet
                .campaign_miseries
                .map_or_else(String::new, |value| value.to_string()),
        }
    }

    pub fn submitted(form: &SheetForm, character: &Character, current: &Sheet) -> Self {
        let mut saved = Self::saved(character, current);
        if let Some(name) = &form.name {
            saved.name.clone_from(name);
        }
        saved.revision.clone_from(&form.revision);
        saved.agility.clone_from(&form.agility);
        saved.presence.clone_from(&form.presence);
        saved.strength.clone_from(&form.strength);
        saved.toughness.clone_from(&form.toughness);
        saved.hp.clone_from(&form.hp);
        saved.max_hp.clone_from(&form.max_hp);
        saved.worn_armor.clone_from(&form.worn_armor);
        saved.protection.clone_from(&form.protection);
        saved.standard_items.clone_from(&form.standard_items);
        saved.silver.clone_from(&form.silver);
        saved.omens.clone_from(&form.omens);
        saved.equipment.clone_from(&form.equipment);
        if let Some(value) = &form.description {
            saved.description.clone_from(value);
        }
        if let Some(value) = &form.class_notes {
            saved.class_notes.clone_from(value);
        }
        if let Some(value) = &form.powers {
            saved.powers.clone_from(value);
        }
        if let Some(value) = &form.weapon_one {
            saved.weapon_one.clone_from(value);
        }
        if let Some(value) = &form.weapon_two {
            saved.weapon_two.clone_from(value);
        }
        if let Some(value) = &form.campaign_miseries {
            saved.campaign_miseries.clone_from(value);
        }
        saved
    }

    pub fn validate(&self) -> Result<Sheet, AppError> {
        let miseries = if self.campaign_miseries.is_empty() {
            None
        } else {
            Some(integer::<u8>(
                &self.campaign_miseries,
                "Campaign miseries reference must be 0–7, or blank for unknown.",
            )?)
        };
        let sheet = Sheet {
            revision: integer(&self.revision, "Expected revision must be an integer.")?,
            agility: integer(&self.agility, "Agility must be an integer.")?,
            presence: integer(&self.presence, "Presence must be an integer.")?,
            strength: integer(&self.strength, "Strength must be an integer.")?,
            toughness: integer(&self.toughness, "Toughness must be an integer.")?,
            hp: integer(&self.hp, "Current HP must be an integer.")?,
            max_hp: integer(&self.max_hp, "Maximum HP must be an integer.")?,
            worn_armor: integer(&self.worn_armor, "Original armor tier must be an integer.")?,
            protection: integer(&self.protection, "Protection tier must be an integer.")?,
            standard_items: integer(&self.standard_items, "Item count must be an integer.")?,
            silver: integer(&self.silver, "Silver must be an integer.")?,
            omens: integer(&self.omens, "Omens must be an integer.")?,
            equipment: self.equipment.clone(),
            description: self.description.clone(),
            class_notes: self.class_notes.clone(),
            powers: self.powers.clone(),
            weapon_one: self.weapon_one.clone(),
            weapon_two: self.weapon_two.clone(),
            campaign_miseries: miseries,
        };
        sheet.validate()?;
        Ok(sheet)
    }
}

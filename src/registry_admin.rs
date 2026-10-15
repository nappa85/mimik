//! Owner-bound registry operations over the shared CLI transitions; prototype/local
//! authority is never inherited and attribution comes only from the admitted session.
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth, with_domain_request},
    pagination::Listing,
    registry::{
        RegisterAlias, RegisterRuleset, RulesetEntry, RulesetKey, alias_in, register_in,
        rulesets_page,
    },
};
use rusqlite::{Connection, TransactionBehavior, params};

impl Store {
    pub(crate) async fn owner_rulesets(
        &self,
        auth: RequestAuth,
        after: Option<RulesetKey>,
    ) -> Result<Listing<RulesetEntry, RulesetKey>, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let page = rulesets_page(&tx, after.as_ref())?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(page)
        })
        .await
    }

    /// Returns whether a new edition was registered (`false` for an exact repeat).
    pub(crate) async fn owner_register_ruleset(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        input: RegisterRuleset,
    ) -> Result<bool, AppError> {
        self.owner_write(auth, csrf, move |db| register_in(db, &input))
            .await
    }

    /// Returns whether a new alias was registered (`false` for an exact repeat).
    pub(crate) async fn owner_register_alias(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        input: RegisterAlias,
    ) -> Result<bool, AppError> {
        self.owner_write(auth, csrf, move |db| alias_in(db, &input))
            .await
    }

    async fn owner_write(
        &self,
        auth: RequestAuth,
        csrf: CsrfAdmission,
        transition: impl FnOnce(&Connection) -> Result<Option<i64>, AppError> + Send + 'static,
    ) -> Result<bool, AppError> {
        self.run_bounded_auth(move |db| {
            with_domain_request(auth.clone(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                let event = transition(&tx)?;
                if let Some(event) = event {
                    tx.execute(
                        "INSERT INTO registry_web_events(event_id,actor_id,actor_epoch,session_digest) VALUES(?1,?2,?3,?4)",
                        params![event, auth.account_id(), auth.epoch(), auth.digest()],
                    )?;
                }
                auth.recheck_owner(&tx)?;
                csrf.recheck(&tx)?;
                tx.commit()?;
                Ok(event.is_some())
            })
        })
        .await
    }
}

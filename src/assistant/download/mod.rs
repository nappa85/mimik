//! Model downloads (assisted sheet plan §1.4b, step 2d-2): the owner-started, hash-verified
//! transfer of a Hugging Face file into the models folder. The only module that uses the HTTP
//! client crate; it takes typed specs and knows nothing of sheets, accounts, campaigns or the
//! database (its rows go through the `Downloads` trait).
pub(crate) mod client;
#[cfg(test)]
pub(crate) mod fake_hub;
#[cfg(test)]
pub(crate) mod fake_proxy;
pub(crate) mod hub;
#[cfg(test)]
mod net_tests;
pub(crate) mod review;

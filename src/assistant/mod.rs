//! The optional interpretation assistant (assisted sheet plan §1.4, step 2c-1).
//!
//! Off by default: without `--assistant` there is no client, no worker and no job route.
pub(crate) mod contract;
pub(crate) mod lexer;
pub(crate) mod output;

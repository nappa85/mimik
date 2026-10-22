//! The optional interpretation assistant (assisted sheet plan §1.4, step 2c-1).
//!
//! Off by default: without `--assistant` there is no client, no worker and no job route.
pub(crate) mod breaker;
pub(crate) mod contract;
#[cfg(test)]
pub(crate) mod fake_server;
pub(crate) mod interpreter;
pub(crate) mod jobs;
pub(crate) mod lexer;
pub(crate) mod output;
pub(crate) mod request;
pub(crate) mod transport;

use std::sync::Arc;

/// The assistant as the binary hands it to the router: an opaque handle. Without one (the
/// default) there is no client, no worker and no job route.
#[derive(Clone)]
pub struct AssistantHandle(pub(crate) Arc<jobs::Assistant>);

impl AssistantHandle {
    /// The post-commit note hook the store calls after every save that wrote notes.
    pub(crate) fn note_hook(&self) -> crate::store::NoteHook {
        let assistant = Arc::clone(&self.0);
        Arc::new(move |sheet, revision, addresses| {
            assistant.note_written(sheet, revision, addresses);
        })
    }

    /// A ready assistant over `interpreter` with `budgets`, its worker running (tests).
    #[cfg(test)]
    pub(crate) fn ready_for_tests(
        interpreter: Arc<dyn interpreter::Interpreter>,
        budgets: jobs::Budgets,
    ) -> Self {
        let assistant = jobs::Assistant::new();
        assistant.event(jobs::RuntimeEvent::GenerationStarted { generation: 1 });
        assistant.event(jobs::RuntimeEvent::Ready {
            generation: 1,
            budgets,
            interpreter,
        });
        tokio::spawn(Arc::clone(&assistant).work());
        Self(assistant)
    }
}

#[cfg(feature = "dev-endpoint")]
pub mod development;

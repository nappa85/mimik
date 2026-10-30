use clap::{Parser, Subcommand};
use mimik::{Store, app};
mod account_cli;

#[derive(Parser)]
#[command(version, about = "Local sheet-first RPG companion")]
struct Args {
    /// Development only: disable authentication and use the isolated loopback prototype.
    #[arg(long)]
    no_auth: bool,
    /// Development only: allow the administrative Owner to use gameplay grants.
    #[arg(long, conflicts_with = "no_auth")]
    dev_owner_play: bool,
    /// Development only: restrict authenticated serving to this machine.
    #[arg(long, conflicts_with = "lan_address")]
    loopback: bool,
    /// Select a local private IPv4 LAN interface instead of automatic selection.
    #[arg(long, conflicts_with = "no_auth")]
    lan_address: Option<std::net::Ipv4Addr>,
    /// Canonical HTTP(S) origin for links/QR, including behind a reverse proxy.
    #[arg(long, conflicts_with = "no_auth")]
    external_url: Option<String>,
    /// `SQLite` database (default: data/mimik.sqlite3, overridden by `MIMIK_DATABASE`).
    #[arg(long, global = true)]
    database: Option<std::path::PathBuf>,
    #[command(flatten)]
    assistant: AssistantArgs,
    #[command(subcommand)]
    command: Option<Command>,
}

/// The interpretation assistant's options.
#[derive(clap::Args)]
struct AssistantArgs {
    /// Turn on the interpretation assistant (off by default): Mimik starts the pinned
    /// llama-server as its child, confined where it can (see /admin/assistant).
    #[arg(long = "assistant", conflicts_with = "no_auth")]
    on: bool,
    /// The model spec (`local:<name>.gguf`, a file in the models folder). Without it the
    /// default depends on the device: Qwen3.5-9B on the GPU path (Granite 4.2 3B if its file
    /// is missing), Granite 4.2 3B on the CPU path. Once the owner has switched models on
    /// /admin/assistant, that stored setting replaces this flag.
    #[arg(long = "assistant-model")]
    model: Option<String>,
    /// The model file's SHA-256 (required for any file but the two defaults).
    #[arg(long = "assistant-model-sha256", requires = "model")]
    model_sha256: Option<String>,
    /// The request's model name, given to the server as `--alias` (default: derived from
    /// the spec's file name).
    #[arg(long = "assistant-model-alias", requires = "model")]
    model_alias: Option<String>,
    /// The models folder (default: `models/` in the data directory; mode 0700).
    #[arg(long = "models-dir")]
    models_dir: Option<std::path::PathBuf>,
    /// The llama.cpp runtime folder (default: `llama/` beside this binary); it must hold
    /// exactly the pinned files.
    #[arg(long = "assistant-runtime")]
    runtime: Option<std::path::PathBuf>,
    /// CPU threads of the model server (default: the physical cores minus one).
    #[arg(long = "assistant-threads")]
    threads: Option<u32>,
    /// The model server's device: `auto` (default: the best hardware GPU, else the CPU),
    /// `cpu`, or a GPU's PCI address such as `0000:0c:00.0`.
    #[arg(long = "assistant-device")]
    device: Option<String>,
    #[cfg(feature = "dev-endpoint")]
    #[command(flatten)]
    development: DevelopmentAssistant,
}

/// The 2c-1 development endpoint (development builds only, never a release; tests).
#[cfg(feature = "dev-endpoint")]
#[derive(clap::Args)]
struct DevelopmentAssistant {
    /// Development only: a hand-started llama-server at `http://127.0.0.1:<port>`,
    /// `http://[::1]:<port>` or an absolute Unix socket path; no endpoint check. It is
    /// primed and calibrated like the runtime's own child.
    #[arg(long = "assistant-dev-endpoint", requires = "on")]
    endpoint: Option<String>,
    /// Development only: the file holding that server's API key.
    #[arg(long = "assistant-dev-token-file", requires = "endpoint")]
    token_file: Option<std::path::PathBuf>,
}

/// The data directory: the database's folder, absolute.
fn data_dir(database: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let folder = database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        );
    std::path::absolute(folder)
}

/// Makes the data folder and its missing parents with an explicit mode of 0700, so the folders
/// Mimik makes do not depend on the umask (Ubuntu's default 002 would make them group-writable).
/// A folder that already exists keeps its mode.
fn create_data_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// The assistant `--assistant` asks for: the runtime manager over the data directory.
async fn assistant(
    args: &Args,
    data_dir: &std::path::Path,
    store: &Store,
) -> Result<mimik::assistant::AssistantHandle, Box<dyn std::error::Error>> {
    #[cfg(feature = "dev-endpoint")]
    if let Some(endpoint) = args.assistant.development.endpoint.clone() {
        let token_file = args
            .assistant
            .development
            .token_file
            .clone()
            .ok_or("--assistant-dev-endpoint needs --assistant-dev-token-file.")?;
        let handle = mimik::assistant::AssistantHandle::development(
            mimik::assistant::development::Config {
                endpoint,
                token_file,
                model: args
                    .assistant
                    .model
                    .clone()
                    .unwrap_or_else(|| mimik::assistant::DEFAULT_MODEL.to_owned()),
                model_sha256: args.assistant.model_sha256.clone(),
                model_alias: args.assistant.model_alias.clone(),
            },
        )?;
        println!("DEVELOPMENT ONLY: assistant on, development endpoint (no endpoint check).");
        return Ok(handle);
    }
    let handle = mimik::assistant::AssistantHandle::runtime(
        mimik::assistant::RuntimeConfig {
            data_dir: data_dir.to_path_buf(),
            models_dir: args.assistant.models_dir.clone(),
            runtime_dir: args.assistant.runtime.clone(),
            model: args.assistant.model.clone(),
            model_sha256: args.assistant.model_sha256.clone(),
            model_alias: args.assistant.model_alias.clone(),
            threads: args.assistant.threads,
            device: args.assistant.device.clone(),
        },
        store.clone(),
    )
    .await?;
    println!("Assistant on: the model server is starting (see /admin/assistant).");
    Ok(handle)
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Account(account_cli::AccountCommand),
    /// Capture a redacted wiring comparison ticket; no automatic approval.
    CompareDefinition {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        published: bool,
    },
    /// Explicitly approve originals compared with the exact current ticket.
    ApproveDefinition {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        published: bool,
        #[arg(long)]
        comparison_hash: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        originals_compared: bool,
    },
    /// Show an immutable published definition and current wiring status.
    ShowDefinitionVersion {
        #[arg(long)]
        id: i64,
    },
    /// Withdraw only the current approval, retaining all history.
    WithdrawDefinition {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        expected_approval: i64,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        rationale: String,
    },
    /// Import a strict proposal as a draft; zero expected revision means create only.
    ImportDefinitionDraft {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        expected_revision: i64,
    },
    /// Show a stored proposal by its positive database row ID; never approves it.
    ShowDefinitionDraft {
        #[arg(long)]
        id: i64,
    },
    /// List up to 100 drafts after an exclusive row-ID cursor.
    ListDefinitionDrafts {
        #[arg(long)]
        game_id: String,
        #[arg(long)]
        edition_id: String,
        #[arg(long, default_value_t = 0)]
        after_id: i64,
    },
    /// Import a strict catalogue as a draft; zero expected revision means create only.
    /// Prints metadata only (ids, revision, hash, counts), never names, keys or text.
    ImportCatalogueDraft {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        expected_revision: i64,
    },
    /// Show a stored catalogue draft's metadata by its positive database row ID.
    ShowCatalogueDraft {
        #[arg(long)]
        id: i64,
    },
    /// Compare a catalogue draft with its latest sealed version: counts and a ticket only.
    CompareCatalogue {
        #[arg(long)]
        id: i64,
    },
    /// Publish a catalogue draft as the next sealed version with the exact current ticket.
    PublishCatalogue {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        comparison_hash: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        rationale: String,
    },
    /// Show a sealed catalogue version's metadata (latest by default) after re-verifying it.
    ShowCatalogueVersion {
        #[arg(long)]
        game: String,
        #[arg(long)]
        edition: String,
        #[arg(long)]
        catalogue: String,
        #[arg(long)]
        version: Option<i64>,
    },
    /// Owner integrity check of every catalogue version and the name index.
    VerifyCatalogues,
    /// Rebuild the derived catalogue name index from the latest sealed versions.
    RebuildCatalogueIndex,
    /// Register a stable game/edition pair; this does not enable a generic sheet.
    RegisterRuleset {
        #[arg(long)]
        game_id: String,
        #[arg(long)]
        game_label: String,
        #[arg(long)]
        edition_id: String,
        #[arg(long)]
        edition_label: String,
    },
    /// Associate an exact legacy edition label with a registered same-game edition.
    RegisterRulesetAlias {
        #[arg(long)]
        game_id: String,
        #[arg(long)]
        edition_id: String,
        #[arg(long)]
        legacy_edition: String,
    },
    /// List stable identities and implemented adapter status as JSON.
    ListRulesets,
    /// Import a prepared source bundle, always unreviewed.
    ImportSource {
        #[arg(long)]
        bundle: std::path::PathBuf,
        #[arg(long)]
        pdf: std::path::PathBuf,
    },
    /// Choose the primary reading for PDF pages (recovers pages left unresolved by overlapping
    /// readings). Recorded as actor `operator:cli`.
    SetPrimarySource {
        /// SHA-256 of the original PDF (lowercase hex), as shown on the source.
        #[arg(long)]
        document_pdf_sha256: String,
        /// PDF pages, for example `3,5-7`.
        #[arg(long)]
        pages: String,
        /// The source (reading) to select; it must cover every page.
        #[arg(long)]
        source: i64,
        #[arg(long)]
        rationale: String,
    },
}

fn main() {
    if let Err(error) = start() {
        // Display, not Debug: real line breaks, no quotes.
        eprintln!("Error: {}", describe(error.as_ref()));
        std::process::exit(1);
    }
}

/// The error's message; for an I/O cause, the operating system's reason too (it names no
/// path or content). Other causes (JSON, sheet data) can quote user text and stay out.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let message = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        if let Some(io) = source.downcast_ref::<std::io::Error>() {
            return format!("{message}: {io}");
        }
        cause = source.source();
    }
    message
}

fn start() -> Result<(), Box<dyn std::error::Error>> {
    // The hidden runner runs before any thread or runtime starts (it sets the parent-death
    // signal and executes in place).
    let raw: Vec<String> = std::env::args().collect();
    if raw.get(1).map(String::as_str) == Some("assistant-runner") {
        std::process::exit(mimik::assistant::runner_main(&raw[2..]));
    }
    if raw.get(1).map(String::as_str) == Some("assistant-apparmor-profile") {
        std::process::exit(mimik::assistant::apparmor_profile_main(&raw[2..]));
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    validate_serving_args(&args)?;
    let auth = !args.no_auth;
    let mut options = server_options(&args, std::env::var("MIMIK_EXTERNAL_URL").ok())?;
    // Absolute once, so every message names the same file.
    let path = database_path(args.database.clone(), std::env::var_os("MIMIK_DATABASE"));
    let path = std::path::absolute(&path).unwrap_or(path);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_data_dir(parent)?;
    }
    let store = Store::open(&path)?;
    // A launch that owner setup refuses starts nothing, the model server included.
    if auth && args.command.is_none() {
        require_owner(&store, &path).await?;
    }
    // Every check that can end the launch comes before the model server starts.
    let unindexed = store.search_index_failures().await?;
    if unindexed > 0 {
        eprintln!(
            "warning: {unindexed} source unit(s) could not be normalised for search and are excluded from lookup; review them as corrected or rejected (see docs/sources-and-retrieval.md)."
        );
    }
    if let Some(command) = args.command {
        return local_command(&store, command).await;
    }
    if args.assistant.on {
        options.assistant = Some(assistant(&args, &data_dir(&path)?, &store).await?);
    }
    let assistant = options.assistant.clone();
    let served = serve(store, auth, options, &path).await;
    if let Some(assistant) = assistant {
        assistant.shutdown().await;
    }
    served
}

/// The local operator commands that print their results.
async fn local_command(store: &Store, command: Command) -> Result<(), Box<dyn std::error::Error>> {
    let store = store.clone();
    let Some(command) = publication_command(&store, command).await? else {
        return Ok(());
    };
    match command {
        Command::ImportDefinitionDraft {
            file,
            expected_revision,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .import_definition_draft(&file, expected_revision)
                    .await?
            )?
        ),
        Command::ShowDefinitionDraft { id } => println!(
            "{}",
            serde_json::to_string_pretty(&store.definition_draft(id).await?)?
        ),
        Command::ListDefinitionDrafts {
            game_id,
            edition_id,
            after_id,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .definition_drafts(game_id, edition_id, after_id)
                    .await?
            )?
        ),
        Command::ImportSource { bundle, pdf } => {
            let id = store.import_source(&bundle, &pdf).await?;
            println!(
                "Registered source {id}. New imports are unreviewed; existing imports retain their review state. Open /sources."
            );
        }
        Command::SetPrimarySource {
            document_pdf_sha256,
            pages,
            source,
            rationale,
        } => {
            let events = store
                .operator_set_primary_source(
                    document_pdf_sha256,
                    mimik::sources::parse_page_list(&pages)?,
                    source,
                    rationale,
                )
                .await?;
            println!("Source {source} selected for {} page(s).", events.len());
        }
        Command::RegisterRuleset {
            game_id,
            game_label,
            edition_id,
            edition_label,
        } => {
            store
                .register_ruleset(mimik::registry::RegisterRuleset {
                    game_id,
                    game_label,
                    edition_id,
                    edition_label,
                })
                .await?;
            println!(
                "Ruleset registered. Registration does not approve sources or provide a generic sheet."
            );
        }
        Command::RegisterRulesetAlias {
            game_id,
            edition_id,
            legacy_edition,
        } => {
            store
                .register_ruleset_alias(mimik::registry::RegisterAlias {
                    game_id,
                    edition_id,
                    legacy_edition,
                })
                .await?;
            println!(
                "Exact legacy alias registered; matching unresolved records associated without changing their data or review state."
            );
        }
        Command::ListRulesets => println!(
            "{}",
            serde_json::to_string_pretty(&store.rulesets().await?)?
        ),
        _ => return Err("Unexpected command dispatcher state".into()),
    }
    Ok(())
}

fn validate_serving_args(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.command.is_some()
        && (args.no_auth
            || args.loopback
            || args.dev_owner_play
            || args.assistant.on
            || args.lan_address.is_some()
            || args.external_url.is_some())
    {
        return Err("Serving options cannot be combined with operator subcommands.".into());
    }
    Ok(())
}
fn database_path(
    explicit: Option<std::path::PathBuf>,
    environment: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    explicit
        .or_else(|| environment.map(Into::into))
        .unwrap_or_else(|| "data/mimik.sqlite3".into())
}

fn server_options(
    args: &Args,
    external_env: Option<String>,
) -> Result<mimik::auth::AuthOptions, Box<dyn std::error::Error>> {
    if args.command.is_some() {
        return Ok(mimik::auth::AuthOptions::default());
    }
    let external_value = args
        .external_url
        .clone()
        .or(external_env.filter(|value| !value.is_empty()));
    if args.no_auth && external_value.is_some() {
        return Err(
            "external_url requires authentication; remove it before using --no-auth.".into(),
        );
    }
    let network = if !args.no_auth && !args.loopback {
        mimik::network::Network::discover(args.lan_address)?
    } else {
        mimik::network::Network::Loopback
    };
    let external_url = if args.no_auth {
        None
    } else {
        external_value
            .map(|value| mimik::network::external_url(&value, network))
            .transpose()?
    };
    Ok(mimik::auth::AuthOptions {
        development_owner_play: args.dev_owner_play,
        assistant: None,
        network,
        external_url,
    })
}

/// Refuses an authenticated launch whose database has no Owner binding, naming the database
/// by its absolute path.
async fn require_owner(
    store: &Store,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match mimik::accounts::Accounts::new(store.clone())
        .require_owner_binding()
        .await
    {
        Ok(()) => Ok(()),
        Err(mimik::AppError::Invalid(mimik::accounts::OWNER_BINDING_REQUIRED)) => {
            Err(format!(
                "Owner setup is required for database {}.\nRun: mimik bootstrap-owner --login owner --display-name Owner --password-file private/owner.password\nUse the same --database or MIMIK_DATABASE setting as this launch. See docs/auth-and-sessions.md for preparing the private password file.",
                path.display()
            )
            .into())
        }
        Err(error) => Err(error.into()),
    }
}

async fn serve(
    store: Store,
    auth: bool,
    options: mimik::auth::AuthOptions,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(options.network.bind_address()).await?;
    if auth {
        return mimik::auth::serve_with_options(listener, store, options).await;
    }
    println!(
        "Mimik unauthenticated prototype — not secured by account setup: http://127.0.0.1:3000 (database: {})",
        path.display()
    );
    axum::serve(listener, app(store))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn publication_command(
    store: &Store,
    command: Command,
) -> Result<Option<Command>, Box<dyn std::error::Error>> {
    match command {
        Command::Account(command) => account_cli::run(store.clone(), command).await?,
        Command::CompareDefinition { id, published } => println!(
            "{}",
            serde_json::to_string_pretty(&store.compare_definition(id, published).await?)?
        ),
        Command::ApproveDefinition {
            id,
            published,
            comparison_hash,
            actor,
            rationale,
            originals_compared,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .approve_definition(
                        id,
                        published,
                        mimik::definitions::ApprovalRequest {
                            comparison_hash,
                            actor,
                            rationale,
                            originals_compared
                        }
                    )
                    .await?
            )?
        ),
        Command::ShowDefinitionVersion { id } => println!(
            "{}",
            serde_json::to_string_pretty(&store.definition_version(id).await?)?
        ),
        Command::WithdrawDefinition {
            id,
            expected_approval,
            actor,
            rationale,
        } => {
            store
                .withdraw_definition(id, expected_approval, actor, rationale)
                .await?;
            println!("Current wiring approval withdrawn; immutable history retained.");
        }
        other => return catalogue_command(store, other).await,
    }
    Ok(None)
}

/// Catalogue drafts print metadata only (plan D6); errors carry static codes.
async fn catalogue_command(
    store: &Store,
    command: Command,
) -> Result<Option<Command>, Box<dyn std::error::Error>> {
    match command {
        Command::ImportCatalogueDraft {
            file,
            expected_revision,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .import_catalogue_draft(&file, expected_revision)
                    .await?
            )?
        ),
        Command::ShowCatalogueDraft { id } => println!(
            "{}",
            serde_json::to_string_pretty(&store.catalogue_draft(id).await?)?
        ),
        Command::CompareCatalogue { id } => println!(
            "{}",
            serde_json::to_string_pretty(&store.compare_catalogue(id).await?)?
        ),
        Command::PublishCatalogue {
            id,
            comparison_hash,
            actor,
            rationale,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .publish_catalogue(
                        id,
                        mimik::catalogues::PublishRequest {
                            comparison_hash,
                            actor,
                            rationale,
                        },
                    )
                    .await?
            )?
        ),
        Command::ShowCatalogueVersion {
            game,
            edition,
            catalogue,
            version,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &store
                    .catalogue_version(game, edition, catalogue, version)
                    .await?
            )?
        ),
        Command::VerifyCatalogues => {
            let report = store.verify_catalogues().await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.problems.is_empty() {
                return Err("Catalogue integrity check failed.".into());
            }
        }
        Command::RebuildCatalogueIndex => println!(
            "{}",
            serde_json::to_string_pretty(&store.rebuild_catalogue_index().await?)?
        ),
        other => return Ok(Some(other)),
    }
    Ok(None)
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;

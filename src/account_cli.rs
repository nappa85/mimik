use clap::Subcommand;
use mimik::{Store, accounts::Accounts};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum AccountCommand {
    /// One-time local operator Owner bootstrap, using exact protected password bytes.
    BootstrapOwner {
        #[arg(long)]
        login: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Reserve a login or supersede its pending invite; output must be a new private file.
    InviteAccount {
        #[arg(long)]
        login: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        token_output: PathBuf,
    },
    /// Enroll using protected credential/password files. No roles are granted.
    EnrollAccount {
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Issue a reset and immediately advance the account's revocation epoch.
    IssueAccountReset {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        expected_epoch: i64,
        #[arg(long)]
        token_output: PathBuf,
    },
    /// Consume a reset and replace the password without granting roles.
    CompleteAccountReset {
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Permanently disable an account. Disabling the Owner ends operator administration.
    DisableAccount {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        expected_epoch: i64,
    },
    /// Show a non-secret account summary under local operator authority.
    ShowAccount {
        #[arg(long)]
        id: i64,
    },
    /// List up to 100 non-secret summaries after an exclusive ID cursor.
    ListAccounts {
        #[arg(long, default_value_t = 0)]
        after_id: i64,
    },
    /// Synthetic/domain password check, not a login or session endpoint.
    VerifyAccountPassword {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        password_file: PathBuf,
    },
}

pub async fn run(store: Store, command: AccountCommand) -> Result<(), Box<dyn std::error::Error>> {
    let accounts = Accounts::new(store);
    let result = match command {
        AccountCommand::BootstrapOwner {
            login,
            display_name,
            password_file,
        } => serde_json::to_value(
            accounts
                .bootstrap(login, display_name, &password_file)
                .await?,
        )?,
        AccountCommand::InviteAccount {
            login,
            display_name,
            token_output,
        } => serde_json::to_value(accounts.invite(login, display_name, &token_output).await?)?,
        AccountCommand::EnrollAccount {
            token_file,
            password_file,
        } => serde_json::to_value(accounts.enroll(&token_file, &password_file).await?)?,
        AccountCommand::IssueAccountReset {
            id,
            expected_epoch,
            token_output,
        } => serde_json::to_value(
            accounts
                .issue_reset(id, expected_epoch, &token_output)
                .await?,
        )?,
        AccountCommand::CompleteAccountReset {
            token_file,
            password_file,
        } => serde_json::to_value(accounts.complete_reset(&token_file, &password_file).await?)?,
        AccountCommand::DisableAccount { id, expected_epoch } => {
            serde_json::to_value(accounts.disable(id, expected_epoch).await?)?
        }
        AccountCommand::ShowAccount { id } => serde_json::to_value(accounts.show(id).await?)?,
        AccountCommand::ListAccounts { after_id } => {
            serde_json::to_value(accounts.list(after_id).await?)?
        }
        AccountCommand::VerifyAccountPassword { id, password_file } => {
            serde_json::json!({"matched":accounts.verify_password(id, &password_file).await?})
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

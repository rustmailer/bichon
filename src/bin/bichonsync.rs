use bichon::modules::account::payload::MinimalAccount;
use bichon::modules::cli::BichonCtlConfig;
use bichon::modules::sync::{
    FetchEmlRequest, MailboxStatusEntry, RawEmailExport, SyncFolderResult, SyncVerifyResult,
};
use clap::{Parser, Subcommand};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use reqwest::Client;
use std::fs;

#[derive(Parser, Debug)]
#[command(
    name = "bichonsync",
    author = "rustmailer",
    version = bichon::bichon_version!(),
    about = "CLI tool for manual sync controls against the Bichon API"
)]
struct Cli {
    /// Path to the configuration file
    #[arg(
        short,
        long,
        default_value = "config.toml",
        value_name = "FILE",
        help = "Sets a custom config file"
    )]
    config: std::path::PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List all accounts
    ListAccounts,
    /// List mailboxes for an account
    ListMailboxes {
        /// Account ID
        #[arg(short, long)]
        account_id: u64,
    },
    /// Trigger a full sync for an account
    Sync {
        /// Account ID to sync
        #[arg(short, long)]
        account_id: u64,
    },
    /// Sync a single folder/mailbox
    SyncFolder {
        /// Account ID
        #[arg(short, long)]
        account_id: u64,
        /// Mailbox ID to sync
        #[arg(short, long)]
        mailbox_id: u64,
        /// Export duplicate/missing messages as .eml files to a tmp directory
        #[arg(long)]
        export_dupes: bool,
    },
    /// Verify sync completeness against the mail server
    Verify {
        /// Account ID to verify
        #[arg(short, long)]
        account_id: u64,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = load_or_prompt_config(&cli.config);
    let client = Client::new();

    match cli.command {
        Commands::ListAccounts => {
            let url = format!("{}/api/v1/minimal-account-list", config.base_url);
            match get_request::<Vec<MinimalAccount>>(&client, &config, &url).await {
                Ok(accounts) => {
                    if accounts.is_empty() {
                        println!("No accounts found.");
                    } else {
                        println!("\n  {:<20} {}", "Account ID", "Email");
                        println!("  {}", "-".repeat(50));
                        for acc in &accounts {
                            println!("  {:<20} {}", acc.id, acc.email);
                        }
                        println!();
                    }
                }
                Err(e) => eprintln!("{} {}", style("✘").red(), e),
            }
        }
        Commands::ListMailboxes { account_id } => {
            let url = format!(
                "{}/api/v1/sync/mailbox-status/{}",
                config.base_url, account_id
            );
            match get_request::<Vec<MailboxStatusEntry>>(&client, &config, &url).await {
                Ok(mailboxes) => {
                    if mailboxes.is_empty() {
                        println!("No mailboxes found for account {}.", account_id);
                    } else {
                        println!(
                            "\n  {:<20} {:<30} {:>10} {:>10}  {}",
                            "Mailbox ID", "Name", "Server", "Local", "Sync"
                        );
                        println!("  {}", "-".repeat(80));
                        for mb in &mailboxes {
                            let sync_icon = if mb.syncing {
                                style("ON").green()
                            } else {
                                style("--").dim()
                            };
                            println!(
                                "  {:<20} {:<30} {:>10} {:>10}  {}",
                                mb.mailbox_id, mb.mailbox_name, mb.server_count, mb.local_count, sync_icon
                            );
                        }
                        println!();
                    }
                }
                Err(e) => eprintln!("{} {}", style("✘").red(), e),
            }
        }
        Commands::Sync { account_id } => {
            println!(
                "{} Triggering sync for account {}...",
                style("⟳").cyan(),
                account_id
            );
            let url = format!("{}/api/v1/sync/{}", config.base_url, account_id);
            match post_request(&client, &config, &url).await {
                Ok(_) => {
                    println!(
                        "{} Sync completed for account {}.",
                        style("✔").green(),
                        account_id
                    );
                    print_mailbox_status(&client, &config, account_id).await;
                }
                Err(e) => eprintln!("{} {}", style("✘").red(), e),
            }
        }
        Commands::SyncFolder {
            account_id,
            mailbox_id,
            export_dupes,
        } => {
            println!(
                "{} Syncing folder {} for account {}...",
                style("⟳").cyan(),
                mailbox_id,
                account_id
            );
            let url = format!(
                "{}/api/v1/sync/{}/{}",
                config.base_url, account_id, mailbox_id
            );
            match post_request_json::<SyncFolderResult>(&client, &config, &url).await {
                Ok(result) => {
                    print_sync_folder_result(&result);
                    if export_dupes && !result.missing_messages.is_empty() {
                        export_missing_emails(
                            &client, &config, account_id, mailbox_id, &result,
                        )
                        .await;
                    }
                    print_mailbox_status(&client, &config, account_id).await;
                }
                Err(e) => eprintln!("{} {}", style("✘").red(), e),
            }
        }
        Commands::Verify { account_id } => {
            println!(
                "{} Verifying sync completeness for account {}...",
                style("⟳").cyan(),
                account_id
            );
            let url = format!("{}/api/v1/sync/verify/{}", config.base_url, account_id);
            match get_request::<SyncVerifyResult>(&client, &config, &url).await {
                Ok(result) => print_verify_result(&result),
                Err(e) => eprintln!("{} {}", style("✘").red(), e),
            }
        }
    }
}

fn load_or_prompt_config(config_path: &std::path::Path) -> BichonCtlConfig {
    let theme = ColorfulTheme::default();

    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(config_path) {
            if let Ok(config) = toml::from_str::<BichonCtlConfig>(&content) {
                println!("{}", style("✔ Using existing configuration:").green());
                println!("  Base URL: {}", style(&config.base_url).yellow());
                return config;
            }
        }
    }

    println!(
        "\n{}",
        style("Please enter Bichon service details:").bold()
    );

    let url: String = Input::with_theme(&theme)
        .with_prompt("Bichon Base URL")
        .default("http://localhost:15630".into())
        .interact_text()
        .unwrap();

    let token: String = Input::with_theme(&theme)
        .with_prompt("API Token")
        .interact_text()
        .unwrap();

    let conf = BichonCtlConfig {
        base_url: url,
        api_token: token,
    };

    if Confirm::with_theme(&theme)
        .with_prompt("Save this configuration for future use?")
        .default(true)
        .interact()
        .unwrap()
    {
        let toml_str = toml::to_string(&conf).unwrap();
        fs::write(config_path, toml_str).expect("Failed to save config file");
        println!("{}", style("Configuration saved.").green());
    }
    conf
}

async fn post_request(
    client: &Client,
    config: &BichonCtlConfig,
    url: &str,
) -> Result<(), String> {
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "No details".into());
        Err(format!("Server error ({}): {}", status, body))
    }
}

async fn post_request_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    config: &BichonCtlConfig,
    url: &str,
) -> Result<T, String> {
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "No details".into());
        return Err(format!("Server error ({}): {}", status, body));
    }

    response
        .json::<T>()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))
}

async fn get_request<T: serde::de::DeserializeOwned>(
    client: &Client,
    config: &BichonCtlConfig,
    url: &str,
) -> Result<T, String> {
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "No details".into());
        return Err(format!("Server error ({}): {}", status, body));
    }

    response
        .json::<T>()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))
}

async fn print_mailbox_status(client: &Client, config: &BichonCtlConfig, account_id: u64) {
    let url = format!(
        "{}/api/v1/sync/mailbox-status/{}",
        config.base_url, account_id
    );
    match get_request::<Vec<MailboxStatusEntry>>(client, config, &url).await {
        Ok(mailboxes) => {
            if mailboxes.is_empty() {
                println!("No mailboxes found for account {}.", account_id);
            } else {
                println!(
                    "\n  {:<20} {:<30} {:>10} {:>10}  {}",
                    "Mailbox ID", "Name", "Server", "Local", "Sync"
                );
                println!("  {}", "-".repeat(80));
                for mb in &mailboxes {
                    let sync_icon = if mb.syncing {
                        style("ON").green()
                    } else {
                        style("--").dim()
                    };
                    println!(
                        "  {:<20} {:<30} {:>10} {:>10}  {}",
                        mb.mailbox_id, mb.mailbox_name, mb.server_count, mb.local_count, sync_icon
                    );
                }
                println!();
            }
        }
        Err(e) => eprintln!("{} Failed to fetch mailbox status: {}", style("✘").red(), e),
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut hash: u32 = 0xFFFFFFFF;
    for &byte in data {
        hash ^= byte as u32;
        for _ in 0..8 {
            hash = if hash & 1 != 0 { (hash >> 1) ^ 0xEDB88320 } else { hash >> 1 };
        }
    }
    !hash
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect()
}

async fn export_missing_emails(
    client: &Client,
    config: &BichonCtlConfig,
    account_id: u64,
    mailbox_id: u64,
    result: &SyncFolderResult,
) {
    let uids: Vec<u32> = result.missing_messages.iter().map(|m| m.uid).collect();
    println!(
        "\n{} Fetching {} EML files from IMAP...",
        style("⟳").cyan(),
        uids.len()
    );

    let url = format!(
        "{}/api/v1/sync/{}/{}/fetch-eml",
        config.base_url, account_id, mailbox_id
    );
    let req_body = FetchEmlRequest { uids };
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&req_body)
        .send()
        .await;

    let exports: Vec<RawEmailExport> = match response {
        Ok(resp) if resp.status().is_success() => match resp.json().await {
            Ok(data) => data,
            Err(e) => {
                eprintln!("{} Failed to parse EML response: {}", style("✘").red(), e);
                return;
            }
        },
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("{} Server error ({}): {}", style("✘").red(), status, body);
            return;
        }
        Err(e) => {
            eprintln!("{} Network error: {}", style("✘").red(), e);
            return;
        }
    };

    let tmp_dir = std::env::temp_dir().join(format!("bichon_export_{}_{}", account_id, mailbox_id));
    if let Err(e) = fs::create_dir_all(&tmp_dir) {
        eprintln!("{} Failed to create tmp dir: {}", style("✘").red(), e);
        return;
    }

    let subject_map: std::collections::HashMap<u32, &str> = result
        .missing_messages
        .iter()
        .map(|m| (m.uid, m.subject.as_str()))
        .collect();

    let mut saved = 0;
    for export in &exports {
        let subject = subject_map.get(&export.uid).copied().unwrap_or("unknown");
        let title_part = sanitize_filename(subject);
        let checksum = crc32(subject.as_bytes());
        let filename = format!("{}_{:08x}_{}.eml", export.uid, checksum, title_part);
        let path = tmp_dir.join(&filename);

        use base64::Engine;
        match base64::engine::general_purpose::STANDARD.decode(&export.eml_base64) {
            Ok(eml_bytes) => {
                if let Err(e) = fs::write(&path, &eml_bytes) {
                    eprintln!("  {} Failed to write {}: {}", style("✘").red(), filename, e);
                } else {
                    println!("  {} {}", style("✔").green(), filename);
                    saved += 1;
                }
            }
            Err(e) => {
                eprintln!("  {} Failed to decode UID {}: {}", style("✘").red(), export.uid, e);
            }
        }
    }

    println!(
        "\n{} Exported {} EML files to {}",
        style("✔").green(),
        saved,
        tmp_dir.display()
    );
}

fn print_sync_folder_result(result: &SyncFolderResult) {
    println!(
        "\n{} Folder sync completed: server={}, local_before={}, local_after={}, missing={}, fetched={}, new={}, dedup={}",
        style("✔").green(),
        result.server_count,
        result.local_count_before,
        result.local_count_after,
        result.missing_count,
        result.fetched,
        result.new_messages,
        result.dedup_count
    );

    if !result.missing_messages.is_empty() {
        println!(
            "\n  {:<8} {:<28} {:<40} {}",
            "UID", "Date", "Message-ID", "Subject"
        );
        println!("  {}", "-".repeat(100));
        for msg in &result.missing_messages {
            let subject_display = if msg.subject.chars().count() > 50 {
                format!("{}...", msg.subject.chars().take(50).collect::<String>())
            } else {
                msg.subject.clone()
            };
            println!(
                "  {:<8} {:<28} {:<40} {}",
                msg.uid, msg.date, format!("<{}>", msg.message_id), subject_display
            );
        }
    }

    if result.dedup_count > 0 {
        println!(
            "\n  {} {} messages share duplicate Message-IDs (dedup expected)",
            style("!").yellow(),
            result.dedup_count
        );
    }
}

fn print_verify_result(result: &SyncVerifyResult) {
    let status = if result.is_complete {
        style("COMPLETE").green().bold()
    } else {
        style("INCOMPLETE").red().bold()
    };
    println!("\nAccount {}: {}", result.account_id, status);

    if !result.missing_folders.is_empty() {
        println!(
            "\n  {} Missing folders (on server but not local):",
            style("!").yellow()
        );
        for folder in &result.missing_folders {
            println!("    - {}", style(folder).yellow());
        }
    }

    if !result.mailboxes.is_empty() {
        println!("\n  Mailbox details:");
        println!(
            "  {:<30} {:>8} {:>8} {:>8}  {}",
            "Name", "Remote", "Local", "Missing", "Status"
        );
        println!("  {}", "-".repeat(78));
        for mb in &result.mailboxes {
            let status_icon = if mb.is_complete {
                style("✔").green()
            } else {
                style("✘").red()
            };
            println!(
                "  {:<30} {:>8} {:>8} {:>8}  {}",
                mb.mailbox_name,
                mb.remote_count,
                mb.local_count,
                mb.missing_count,
                status_icon
            );
        }
    }
    println!();
}

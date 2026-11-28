use crate::BichonCliConfig;
use bichon_core::{
    base64_encode, envelope::meta::BichonMetadata, export::escape_from_lines,
    store::envelope::Envelope,
};
use chrono::{TimeZone, Utc};
use reqwest::Client;
use tokio::io::AsyncWriteExt;

pub async fn download_and_export_with_json_header(
    client: &Client,
    config: &BichonCliConfig,
    envelope: Envelope,
    file: &mut tokio::fs::File,
) -> bool {
    let url = format!(
        "{}/api/v1/download-message/{}/{}",
        config.base_url, &envelope.account_id, &envelope.id
    );

    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
    {
        Ok(res) => {
            if !res.status().is_success() {
                eprintln!(
                    " ✘ HTTP Error {}: Failed for {}",
                    res.status(),
                    &envelope.id
                );
                return false;
            }
            res
        }
        Err(e) => {
            eprintln!(" ✘ Network error: {} for {}", e, &envelope.id);
            return false;
        }
    };

    let email_bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                " ✘ Failed to read response body for {}: {}",
                &envelope.id, e
            );
            return false;
        }
    };

    let date_dt = Utc.timestamp_opt(envelope.date / 1000, 0).unwrap();
    let date_str = date_dt.format("%a %b %e %H:%M:%S %Y").to_string();
    let from_line = format!("From {} {}\n", envelope.from.clone(), date_str);

    let custom_header = build_metadata_header(BichonMetadata {
        account_email: envelope.account_email,
        mailbox_name: envelope.mailbox_name,
        tags: envelope.tags,
    });

    let mut final_buffer =
        Vec::with_capacity(from_line.len() + custom_header.len() + email_bytes.len() + 2);
    final_buffer.extend_from_slice(from_line.as_bytes());
    final_buffer.extend_from_slice(custom_header.as_bytes());
    final_buffer.extend_from_slice(&escape_from_lines(&email_bytes));
    final_buffer.extend_from_slice(b"\n\n");

    if let Err(e) = file.write_all(&final_buffer).await {
        eprintln!(" ✘ IO Error: Failed to write to mbox: {}", e);
        return false;
    }

    true
}

fn build_metadata_header(meta: BichonMetadata) -> String {
    let json_str = serde_json::to_string(&meta).ok().unwrap();
    let encoded = base64_encode!(json_str);
    format!("X-Bichon-Metadata: {}\r\n", encoded)
}

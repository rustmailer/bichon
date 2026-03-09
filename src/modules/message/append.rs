use crate::{
    encode_mailbox_name,
    modules::{
        account::migration::{AccountModel, AccountType},
        error::{code::ErrorCode, BichonResult},
        imap::executor::ImapExecutor,
        indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER},
        utils::create_hash,
    },
    raise_error,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

const MAX_RESTORE_COUNT: usize = 100;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct RestoreMessagesRequest {
    /// Message IDs to restore (max 100)
    pub message_ids: Vec<u64>,
}

pub async fn restore_emails(account_id: u64, message_ids: Vec<u64>) -> BichonResult<()> {
    if message_ids.len() > MAX_RESTORE_COUNT {
        return Err(raise_error!(
            format!(
                "Too many messages to restore: {} (max {})",
                message_ids.len(),
                MAX_RESTORE_COUNT
            ),
            ErrorCode::InvalidParameter
        ));
    }

    let account = AccountModel::check_account_exists(account_id).await?;
    if !matches!(account.account_type, AccountType::IMAP) {
        return Err(raise_error!(
            "Account type is not IMAP".into(),
            ErrorCode::Incompatible
        ));
    }

    let mut failed = Vec::new();
    let mut session = ImapExecutor::create_connection(account_id).await?;
    for message_id in message_ids {
        let result: BichonResult<()> = async {
            let envelope = ENVELOPE_INDEX_MANAGER
                .get_envelope_by_id(account_id, message_id)
                .await?
                .ok_or_else(|| {
                    raise_error!(
                        format!(
                            "Envelope not found: account_id={} message_id={}",
                            account_id, message_id
                        ),
                        ErrorCode::ResourceNotFound
                    )
                })?;
            let eml_id = create_hash(account_id, &envelope.message_id);
            let eml = EML_INDEX_MANAGER
                .get(account_id, eml_id)
                .await?
                .ok_or_else(|| {
                    raise_error!(
                        format!("Eml not found: account_id={} id={}", account_id, message_id),
                        ErrorCode::ResourceNotFound
                    )
                })?;

            if let Some(mailbox_name) = envelope.mailbox_name {
                ImapExecutor::append(
                    &mut session,
                    encode_mailbox_name!(&mailbox_name),
                    None,
                    None,
                    &eml,
                )
                .await?;
            }

            Ok(())
        }
        .await;

        if let Err(err) = result {
            failed.push(message_id);
            tracing::warn!(
                account_id = account_id,
                message_id = message_id,
                error = ?err,
                "Failed to restore email"
            );
        }
    }

    if !failed.is_empty() {
        tracing::info!(
            account_id = account_id,
            failed_count = failed.len(),
            failed_message_ids = ?failed,
            "Restore emails finished with partial failures"
        );
    }

    session.logout().await.ok();

    Ok(())
}

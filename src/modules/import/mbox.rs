use mbox_reader::MboxFile as Mbox;
use std::path::Path;
use tantivy::doc;

use crate::{
    modules::{
        account::migration::{AccountModel, AccountType},
        cache::imap::mailbox::{Attribute, AttributeEnum, MailBox},
        envelope::extractor::extract_envelope_from_eml,
        error::{code::ErrorCode, BichonResult},
        indexer::{
            manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER},
            schema::SchemaTools,
        },
        mbox::migration::MboxFileModel as MboxFile,
        utils::create_hash,
    },
    raise_error,
};

pub async fn process_mbox_message(
    eml_bytes: &[u8],
    account_id: u64,
    mailbox_id: u64,
    mbox_id: u64,
    offset: u64,
    len: u64,
) -> BichonResult<()> {
    let envelope = extract_envelope_from_eml(eml_bytes, account_id, mailbox_id)?;

    ENVELOPE_INDEX_MANAGER
        .add_document(envelope.id, envelope.to_document(mailbox_id).unwrap())
        .await;

    let fields = SchemaTools::eml_fields();
    EML_INDEX_MANAGER
        .add_document(
            envelope.id,
            doc!(
                fields.f_id => envelope.id,
                fields.f_account_id => account_id,
                fields.f_mailbox_id => mailbox_id,
                fields.f_mbox_id => mbox_id,
                fields.f_mbox_offset => offset,
                fields.f_mbox_len => len,
            ),
        )
        .await;

    Ok(())
}

pub async fn import_mbox_from_path(
    path: &Path,
    account_id: u64,
    mail_folder: String,
) -> BichonResult<()> {
    // 1. Ensure the account exists
    let account = AccountModel::check_account_exists(account_id).await?;

    // 2. Register the mbox file and get its ID
    let mbox_file = MboxFile::register(path.to_string_lossy().to_string(), account.id).await?;

    // 3. Create or find the mailbox
    let mailbox_id = match account.account_type {
        AccountType::IMAP => {
            // For IMAP accounts, the mailbox must exist.
            let all_mailboxes = MailBox::list_all(account.id).await?;
            let mailbox = all_mailboxes.into_iter().find(|m| m.name == mail_folder);
            match mailbox {
                Some(mailbox) => mailbox.id,
                None => return Err(raise_error!(
                    format!("Mail folder '{}' not found for account ID {}. The target folder must exist before importing.", 
                            mail_folder, 
                            account.id).into(),
                    ErrorCode::ResourceNotFound
                )),
            }
        },
        AccountType::NoSync => {
            // For NoSync accounts, we can create the mailbox.
            let mailbox = MailBox {
                id: create_hash(account.id, &mail_folder),
                account_id: account.id,
                name: mail_folder.clone(),
                delimiter: Some("/".to_string()),
                attributes: vec![Attribute {
                    attr: AttributeEnum::Extension,
                    extension: Some("CreatedByBichon".into()),
                }],
                exists: 0,
                unseen: None,
                uid_next: None,
                uid_validity: None,
            };
            let mailbox_id = mailbox.id;
            MailBox::batch_upsert(&[mailbox]).await?;
            mailbox_id
        },
    };

    // 4. Open the mbox file
    let mbox = Mbox::from_file(path).map_err(|e| {
        raise_error!(
            format!("Failed to open mbox file: {}", e),
            ErrorCode::IoError
        )
    })?;

    // 5. Iterate over messages and process them
    for message in mbox.iter() {
        let offset = message.offset() as u64;
        let bytes = match message.message() {
            Some(b) => b,
            None => continue,
        };
        let len = bytes.len() as u64;

        if let Err(e) =
            process_mbox_message(bytes, account.id, mailbox_id, mbox_file.id, offset, len).await
        {
            tracing::error!(
                "Failed to process message from mbox at offset {}: {:?}",
                offset,
                e
            );
        }
    }

    Ok(())
}
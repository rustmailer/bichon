use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

use crate::modules::account::migration::{AccountModel, AccountType};
use crate::modules::error::{BichonResult, code::ErrorCode};
use crate::modules::import::mbox::import_mbox_from_path;
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use crate::modules::message::content::retrieve_email_content;
use crate::raise_error;

#[tokio::test]
async fn test_import_mbox_with_attachment() -> BichonResult<()> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    std::env::set_var("BICHON_DATA_DIR", &data_dir);

    let mbox_content = b"From MAILER-DAEMON Fri Jul  8 12:08:34 2011
From: John Doe <john.doe@example.com>
To: Mary Roe <mary.roe@example.com>
Subject: Sample message 1
Date: Fri, 8 Jul 2011 12:08:34 -0500 (CDT)

This is a sample message.
From MAILER-DAEMON Fri Jul  8 12:08:34 2011
From: John Doe <john.doe@example.com>
To: Mary Roe <mary.roe@example.com>
Subject: Sample message 2 with attachment
Date: Fri, 8 Jul 2011 12:08:34 -0500 (CDT)
Content-Type: multipart/mixed; boundary=\"boundary\"

--boundary
Content-Type: text/plain; charset=us-ascii

This is a sample message with an attachment.
--boundary
Content-Type: text/plain; charset=us-ascii
Content-Disposition: attachment; filename=\"test.txt\"

This is the attachment.
--boundary--
";

    let mbox_path = temp_dir.path().join("test.mbox");
    let mut file = File::create(&mbox_path)?;
    file.write_all(mbox_content)?;

    let account = AccountModel {
        id: 1,
        email: "test@example.com".to_string(),
        account_type: AccountType::NoSync,
        ..Default::default()
    };
    account.save().await?;

    import_mbox_from_path(&mbox_path, account.id, "imported_from_mbox".to_string()).await?;

    // We need a small delay to allow the indexer to process the messages.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Search for the messages to get their IDs
    let searcher = ENVELOPE_INDEX_MANAGER.create_searcher()?;
    let query = ENVELOPE_INDEX_MANAGER.account_query(account.id);
    let top_docs = searcher.search(query.as_ref(), &tantivy::collector::TopDocs::with_limit(10))
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?;

    assert_eq!(top_docs.len(), 2, "Expected to find 2 messages");

    let mut found_attachment = false;
    for (_score, doc_address) in top_docs {
        let doc = searcher.doc(doc_address)
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?;
        let envelope = crate::modules::indexer::envelope::Envelope::from_tantivy_doc(&doc).await?;
        let content = retrieve_email_content(account.id, envelope.id).await?;
        if let Some(attachments) = content.attachments {
            if !attachments.is_empty() {
                assert_eq!(attachments[0].filename, "test.txt");
                let eml = EML_INDEX_MANAGER.get(account.id, envelope.id).await?.unwrap();
                let parsed = mail_parser::MessageParser::default().parse(&eml).unwrap();
                let attachment_content = parsed.attachments().next().unwrap().contents();
                assert_eq!(attachment_content, b"This is the attachment.");
                found_attachment = true;
            }
        }
    }

    assert!(found_attachment, "Did not find the attachment");

    Ok(())
}

use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tantivy::doc;

use crate::{
    base64_decode_url_safe,
    modules::{
        account::migration::{AccountModel, AccountType},
        cache::imap::mailbox::{Attribute, AttributeEnum, MailBox},
        envelope::extractor::extract_envelope_from_eml,
        error::{code::ErrorCode, BichonResult},
        indexer::{
            manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER},
            schema::SchemaTools,
        },
        utils::create_hash,
    },
    raise_error,
};

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct BatchEmlRequest {
    pub account_id: u64,
    pub mail_folder: String,
    /// A list of emails in base64-encoded format. Each element represents one .eml file.
    pub emls: Vec<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct FailedEmlDetail {
    /// The 0-based index of the failed EML in the request list
    pub index: usize,
    /// The error message that caused the import to fail
    pub error_message: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct BatchEmlResult {
    /// Total number of emails processed
    pub total: usize,
    /// Number of emails successfully imported
    pub success: usize,
    /// Number of emails failed to import
    pub failed: usize,
    /// A list of details for failed imports
    pub failed_details: Vec<FailedEmlDetail>,
}

pub struct ImportEmls;

impl ImportEmls {
    pub async fn do_import(request: BatchEmlRequest) -> BichonResult<BatchEmlResult> {
        let account = AccountModel::check_account_exists(request.account_id).await?;
        
        if !account.enabled {
            return Err(raise_error!("The account is disabled and cannot be used for this operation.".into(), ErrorCode::InvalidParameter));
        }

        let mailbox_id = match account.account_type {
            AccountType::IMAP => {
                let all_mailboxes = MailBox::list_all(account.id).await?;
                let mailbox = all_mailboxes.into_iter().find(|m| m.name == request.mail_folder);
                
                match mailbox {
                    Some(mailbox) => mailbox.id,
                    None => return Err(raise_error!(
                        format!("Mail folder '{}' not found for account ID {}. The target folder must exist before importing.", 
                                request.mail_folder, 
                                request.account_id).into(),
                        ErrorCode::ResourceNotFound
                    )),
                }
            },
            AccountType::NoSync => {
                let mailbox = MailBox {
                    id: create_hash(request.account_id, &request.mail_folder),
                    account_id: request.account_id,
                    name: request.mail_folder.clone(),
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
                // Upsert the mailbox, creating it if it doesn't exist
                MailBox::batch_upsert(&[mailbox]).await?;
                mailbox_id
            },
        };

        let fields = SchemaTools::eml_fields();
        let account_id = account.id;
        let mut success_count = 0;
        let mut failed_details: Vec<FailedEmlDetail> = Vec::new(); // Store failure details

        let total = request.emls.len();
        for (index, eml_base64) in request.emls.into_iter().enumerate() {
            let decoded = match base64_decode_url_safe!(eml_base64.as_bytes()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let error_msg =
                        format!("Failed to decode base64 EML at index {}: {:?}", index, e);
                    tracing::error!("{}", error_msg);
                    failed_details.push(FailedEmlDetail {
                        index,
                        error_message: error_msg,
                    });
                    continue;
                }
            };

            let envelope = match extract_envelope_from_eml(&decoded, account_id, mailbox_id) {
                Ok(env) => env,
                Err(e) => {
                    let error_msg = format!(
                        "Failed to extract envelope from EML at index {}: {:?}",
                        index, e
                    );
                    tracing::error!("{}", error_msg);
                    failed_details.push(FailedEmlDetail {
                        index,
                        error_message: error_msg,
                    });
                    continue;
                }
            };
            let eml_id = create_hash(account_id, &envelope.0.message_id);
            ENVELOPE_INDEX_MANAGER
                .add_document(envelope.0.id, envelope)
                .await;

            EML_INDEX_MANAGER
                .add_document(
                    eml_id,
                    doc!(
                        fields.f_id => eml_id,
                        fields.f_account_id => account_id,
                        fields.f_mailbox_id => mailbox_id,
                        fields.f_eml => decoded
                    ),
                )
                .await;

            success_count += 1;
        }

        let failed_count = failed_details.len();

        Ok(BatchEmlResult {
            total,
            success: success_count,
            failed: failed_count,
            failed_details, // Return the list of failure details
        })
    }
}

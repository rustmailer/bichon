use std::io::Cursor;

use crate::{
    raise_error,
    {
        dashboard::Group,
        envelope::extractor::{
            reattach_eml_content, reattach_eml_content_self_healing, recover_message_blob,
        },
        error::{code::ErrorCode, BichonResult},
        store::blob::BLOB_MANAGER,
        utils::compute_content_hash,
    },
};
use bytes::Bytes;
use mail_parser::MessageParser;
//use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AttachmentMetadata {
    /// Statistics of attachment file extensions (key + count).
    /// Each item represents a file extension and its occurrence count.
    /// Example: [{ key: "pdf", count: 10 }, { key: "png", count: 5 }]
    pub extensions: Vec<Group>,

    /// Statistics of attachment categories (key + count).
    /// Each item represents a high-level category and its occurrence count.
    /// Example: [{ key: "document", count: 8 }, { key: "image", count: 6 }]
    pub categories: Vec<Group>,

    /// Statistics of attachment MIME types (Content-Type) (key + count).
    /// Each item represents a MIME type and its occurrence count.
    /// Example: [{ key: "application/pdf", count: 10 }, { key: "image/jpeg", count: 5 }]
    pub content_types: Vec<Group>,
}

pub fn retrieve_attachment_content(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
) -> BichonResult<Cursor<Bytes>> {
    let (_, eml) = reattach_eml_content(account_id, envelope_id)?;
    let message = MessageParser::default()
        .parse(&eml)
        .ok_or_else(|| raise_error!("Failed to parse EML".into(), ErrorCode::InternalError))?;

    let attachment_content: &[u8] = message
        .attachments()
        .find(|att| compute_content_hash(att.contents()) == content_hash)
        .map(|att| att.contents())
        .ok_or_else(|| {
            raise_error!(
                "Target attachment not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;
    Ok(Cursor::new(Bytes::copy_from_slice(attachment_content)))
}

pub async fn retrieve_attachment_content_self_healing(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
) -> BichonResult<Cursor<Bytes>> {
    let (envelope, eml) = reattach_eml_content_self_healing(account_id, envelope_id).await?;

    let indexed_attachment = envelope_has_attachment(account_id, &envelope.id, content_hash)?;
    let detached_placeholder = eml_contains_detached_attachment_placeholder(&eml, content_hash);
    if indexed_attachment || detached_placeholder {
        if let Some(content) = BLOB_MANAGER.get_decoded_attachment(content_hash)? {
            return Ok(Cursor::new(content));
        }
    }

    match find_attachment_content(&eml, content_hash) {
        Ok(Some(content)) => return Ok(Cursor::new(content)),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                account_id,
                envelope_id = %envelope.id,
                content_hash,
                error = %error,
                "Reconstructed message attachment lookup failed; attempting IMAP recovery"
            );
        }
    }

    if envelope.regular_attachment_count > 0 {
        tracing::warn!(
            account_id,
            envelope_id = %envelope.id,
            content_hash,
            expected_regular_attachments = envelope.regular_attachment_count,
            "Reconstructed message did not contain requested attachment; refetching archived message from IMAP"
        );
        let raw_body = recover_message_blob(&envelope).await?;
        if indexed_attachment
            || detached_placeholder
            || eml_contains_detached_attachment_placeholder(&raw_body, content_hash)
        {
            if let Some(content) = BLOB_MANAGER.get_decoded_attachment(content_hash)? {
                return Ok(Cursor::new(content));
            }
        }
        if let Some(content) = find_attachment_content(&raw_body, content_hash)? {
            return Ok(Cursor::new(content));
        }
    }

    Err(raise_error!(
        "Target attachment not found".into(),
        ErrorCode::ResourceNotFound
    ))
}

fn envelope_has_attachment(
    account_id: u64,
    envelope_id: &str,
    content_hash: &str,
) -> BichonResult<bool> {
    let Some(envelope) = crate::store::tantivy::envelope::ENVELOPE_MANAGER
        .get_envelope_by_id(account_id, envelope_id)?
    else {
        return Ok(false);
    };
    Ok(envelope
        .attachments
        .unwrap_or_default()
        .iter()
        .any(|attachment| attachment.content_hash == content_hash))
}

fn eml_contains_detached_attachment_placeholder(eml: &[u8], content_hash: &str) -> bool {
    let placeholder = format!("<<BICHON_DETACH_HASH:{content_hash}>>");
    eml.windows(placeholder.len())
        .any(|window| window == placeholder.as_bytes())
}

fn find_attachment_content(eml: &[u8], content_hash: &str) -> BichonResult<Option<Bytes>> {
    let message = MessageParser::default()
        .parse(eml)
        .ok_or_else(|| raise_error!("Failed to parse EML".into(), ErrorCode::InternalError))?;

    let content = message
        .attachments()
        .find(|att| compute_content_hash(att.contents()) == content_hash)
        .map(|att| Bytes::copy_from_slice(att.contents()));
    Ok(content)
}

pub fn retrieve_nested_attachment_content(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
    nested_content_hash: &str,
) -> BichonResult<Cursor<Bytes>> {
    let (_, eml) = reattach_eml_content(account_id, envelope_id)?;
    let parent_message = MessageParser::default().parse(&eml).ok_or_else(|| {
        raise_error!(
            "Failed to parse parent EML".into(),
            ErrorCode::InternalError
        )
    })?;

    let attachment_content = parent_message
        .attachments()
        .find(|att| compute_content_hash(att.contents()) == content_hash)
        .map(|att| att.contents())
        .ok_or_else(|| {
            raise_error!(
                "Target nested EML not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;

    let nested_message = MessageParser::default()
        .parse(attachment_content)
        .ok_or_else(|| {
            raise_error!(
                "Failed to parse nested EML".into(),
                ErrorCode::InternalError
            )
        })?;

    let attachment_content = nested_message
        .attachments()
        .find(|att| compute_content_hash(att.contents()) == nested_content_hash)
        .map(|att| att.contents())
        .ok_or_else(|| {
            raise_error!(
                "Target nested EML not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;

    Ok(Cursor::new(Bytes::copy_from_slice(attachment_content)))
}

#[cfg(test)]
mod tests {
    use super::eml_contains_detached_attachment_placeholder;

    #[test]
    fn detects_exact_detached_attachment_placeholder() {
        let hash = "12fc5a986e7f358daf4536cbc414757458b55c9b25333f2c9c868884735f0554";
        let eml = format!("Content-Type: application/pdf\r\n\r\n<<BICHON_DETACH_HASH:{hash}>>\r\n");

        assert!(eml_contains_detached_attachment_placeholder(
            eml.as_bytes(),
            hash
        ));
        assert!(!eml_contains_detached_attachment_placeholder(
            eml.as_bytes(),
            "12fc5a986e7f358daf4536cbc414757458b55c9b25333f2c9c868884735f0555"
        ));
    }
}

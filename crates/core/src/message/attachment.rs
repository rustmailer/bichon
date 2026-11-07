use std::io::Cursor;

use crate::{
    raise_error,
    {
        dashboard::Group,
        envelope::extractor::{part_contents, reattach_eml_content},
        error::{code::ErrorCode, BichonResult},
        message::content::AttachmentInfo,
        store::{blob::BLOB_MANAGER, tantivy::envelope::ENVELOPE_MANAGER},
        utils::compute_content_hash,
    },
};
use bytes::Bytes;
use mail_parser::{Message, MessageParser};
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

/// Decoded attachment bytes from the blob store, for records written before
/// #375.
///
/// Those blobs hold the *decoded* contents, so they can be returned as-is.
/// Current records keep the encoded MIME body (needed to rebuild the original
/// EML losslessly), which is why they still have to be re-parsed.
fn fetch_legacy_attachment(
    blobs: &crate::store::blob::BlobManager,
    attachments: Option<&[AttachmentInfo]>,
    content_hash: &str,
) -> BichonResult<Option<Bytes>> {
    let Some(attachment) = attachments
        .and_then(|list| list.iter().find(|a| a.content_hash == content_hash))
    else {
        return Ok(None);
    };
    if attachment.raw_content_hash.is_some() {
        return Ok(None);
    }
    blobs.get_attachment(attachment.storage_hash())
}

/// The attachment's stored bytes, if the envelope still carries a legacy
/// attachment record.
fn stored_attachment(
    account_id: u64,
    envelope_id: &str,
    content_hash: &str,
) -> BichonResult<Option<Bytes>> {
    let Some(envelope) = ENVELOPE_MANAGER.get_envelope_by_id(account_id, envelope_id)? else {
        return Ok(None);
    };
    fetch_legacy_attachment(&BLOB_MANAGER, envelope.attachments.as_deref(), content_hash)
}

/// The part whose contents hash to `content_hash`, across the whole message.
///
/// Deliberately not restricted to `attachment_parts`: attachment identity is
/// the stored hash, and a later parser version may classify the same part as
/// body text when it re-parses the message. Searching every non-multipart part
/// keeps such a record downloadable instead of returning "not found" for bytes
/// that are still archived.
fn find_part_contents<'a>(
    message: &'a Message<'_>,
    content_hash: &str,
) -> Option<std::borrow::Cow<'a, [u8]>> {
    message
        .root()
        .parts()
        .filter(|part| !matches!(part.kind(), mail_parser::PartKind::Multipart))
        .find(|part| compute_content_hash(&part_contents(part)) == content_hash)
        .map(|part| part_contents(&part))
}

pub fn retrieve_attachment_content(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
) -> BichonResult<Cursor<Bytes>> {
    // Pre-#375 records can be served straight from their decoded blob, which
    // keeps them independent of how a newer parser reads the EML.
    if let Some(bytes) = stored_attachment(account_id, &envelope_id, content_hash)? {
        return Ok(Cursor::new(bytes));
    }

    let (_, eml) = reattach_eml_content(account_id, envelope_id)?;
    let message = MessageParser::default()
        .parse(&eml)
        .ok_or_else(|| raise_error!("Failed to parse EML".into(), ErrorCode::InternalError))?;

    let attachment_content = find_part_contents(&message, content_hash).ok_or_else(|| {
        raise_error!(
            "Target attachment not found".into(),
            ErrorCode::ResourceNotFound
        )
    })?;
    Ok(Cursor::new(Bytes::copy_from_slice(
        attachment_content.as_ref(),
    )))
}

pub fn retrieve_nested_attachment_content(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
    nested_content_hash: &str,
) -> BichonResult<Cursor<Bytes>> {
    // The outer attachment is a stored blob as well; read it straight from
    // the frozen list when possible. Only the inner part still needs parsing,
    // because parts inside an attached EML are never detached separately.
    let attachment_content: Bytes =
        match stored_attachment(account_id, &envelope_id, content_hash)? {
            Some(bytes) => bytes,
            None => {
                let (_, eml) = reattach_eml_content(account_id, envelope_id)?;
                let parent_message = MessageParser::default().parse(&eml).ok_or_else(|| {
                    raise_error!(
                        "Failed to parse parent EML".into(),
                        ErrorCode::InternalError
                    )
                })?;
                let found = find_part_contents(&parent_message, content_hash).ok_or_else(|| {
                    raise_error!(
                        "Target nested EML not found".into(),
                        ErrorCode::ResourceNotFound
                    )
                })?;
                Bytes::copy_from_slice(found.as_ref())
            }
        };

    let nested_message = MessageParser::default()
        .parse(&attachment_content)
        .ok_or_else(|| {
            raise_error!(
                "Failed to parse nested EML".into(),
                ErrorCode::InternalError
            )
        })?;

    let attachment_content =
        find_part_contents(&nested_message, nested_content_hash).ok_or_else(|| {
            raise_error!(
                "Target nested EML not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;

    Ok(Cursor::new(Bytes::copy_from_slice(
        attachment_content.as_ref(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::extractor::attachment_parts;
    use crate::store::blob::{BlobManager, DetachedEmail};

    fn info(content_hash: &str, raw_content_hash: Option<&str>) -> AttachmentInfo {
        AttachmentInfo {
            content_hash: content_hash.to_string(),
            raw_content_hash: raw_content_hash.map(str::to_string),
            ..AttachmentInfo::default()
        }
    }

    /// Only legacy blobs hold decoded contents. Current records store the
    /// encoded MIME body, so they must go through the re-parse path.
    #[test]
    fn fetch_legacy_attachment_serves_only_decoded_blobs() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let root = crate::test_support::unique_test_root("bichon-attachment-fetch");
        let dir = root.join("blobs");

        runtime.block_on(async {
            let blobs = BlobManager::open(dir);
            let legacy_hash = compute_content_hash(b"decoded attachment body");
            let encoded_hash = compute_content_hash(b"encoded MIME body");
            let bytes = Bytes::from_static(b"attachment bytes");

            blobs
                .queue(DetachedEmail {
                    email: (compute_content_hash(b"email"), Bytes::from_static(b"email")),
                    attachments: Some(vec![(legacy_hash.clone(), bytes.clone())]),
                })
                .await;
            blobs.drain().await;

            // Legacy record: content_hash is the blob key and holds the
            // decoded bytes, so it is served directly.
            let legacy = info(&legacy_hash, None);
            assert_eq!(
                fetch_legacy_attachment(&blobs, Some(&[legacy]), &legacy_hash).unwrap(),
                Some(bytes)
            );

            // Current record: the blob is the encoded body, never returned
            // as-is.
            let current = info(&encoded_hash, Some(&encoded_hash));
            assert_eq!(
                fetch_legacy_attachment(&blobs, Some(&[current]), &encoded_hash).unwrap(),
                None
            );

            // Unknown hashes and missing metadata fall through to the
            // re-parse path.
            let legacy = info(&legacy_hash, None);
            assert_eq!(
                fetch_legacy_attachment(&blobs, Some(&[legacy]), "missing").unwrap(),
                None
            );
            assert_eq!(
                fetch_legacy_attachment(&blobs, None, &legacy_hash).unwrap(),
                None
            );

            // Drop the sender so the writer task exits before the directory is
            // removed (Windows keeps open handles locked).
            drop(blobs);
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        });

        let _ = std::fs::remove_dir_all(root);
    }

    /// A part that 1.0 classifies as body text is still found by its stored
    /// hash. This is the regression that used to turn a download into 404
    /// after an upgrade: 0.11 archived the unterminated part, 1.0 calls it
    /// body, and the old lookup only searched `attachment_parts`.
    #[test]
    fn find_part_contents_reaches_parts_the_parser_now_calls_body() {
        let raw = concat!(
            "Content-Type: multipart/mixed; boundary=\"b\"\r\n",
            "\r\n",
            "--b\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "no end\r\n",
        )
        .as_bytes();

        let message = MessageParser::default().parse(raw).expect("fixture parses");
        assert!(attachment_parts(&message).next().is_none());

        let body_part = message
            .root()
            .parts()
            .find(|part| part.in_text_body())
            .expect("the unterminated part is body text");
        let contents = part_contents(&body_part);
        let hash = compute_content_hash(&contents);

        assert_eq!(
            find_part_contents(&message, &hash).as_deref(),
            Some(contents.as_ref())
        );
    }
}

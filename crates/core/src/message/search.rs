//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{
    common::auth::ClientContext,
    common::paginated::DataPage,
    error::{code::ErrorCode, BichonResult},
    message::contacts::Contact,
    raise_error,
    store::{
        envelope::Envelope,
        tantivy::{
            attachment::ATTACHMENT_MANAGER,
            envelope::ENVELOPE_MANAGER,
            model::{merge_contact, AttachmentModel},
        },
    },
};

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct EmailSearchFilter {
    pub text: Option<String>,
    /// Whether every whitespace-separated term must match (`true`, the current
    /// default) or any of them (`false`). `None` means the default. Legacy saved
    /// searches are replayed with `false` so they keep the semantics they were
    /// authored under.
    #[serde(default)]
    pub match_all: Option<bool>,
    pub subject: Option<String>,
    pub id: Option<String>,
    pub body: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    /// Matches if the address appears in `to`, `cc`, or `bcc` (OR semantics).
    pub any_recipient: Option<String>,
    /// Matches if the address appears in `from`, `to`, `cc`, or `bcc` (OR semantics).
    pub any_participant: Option<String>,
    pub since: Option<i64>,
    pub before: Option<i64>,
    /// Lower bound (inclusive) on the IMAP server INTERNALDATE timestamp.
    pub internal_date_since: Option<i64>,
    /// Upper bound (inclusive) on the IMAP server INTERNALDATE timestamp.
    pub internal_date_before: Option<i64>,
    /// Lower bound (inclusive) on Bichon's archival (ingest) timestamp.
    pub ingest_since: Option<i64>,
    /// Upper bound (inclusive) on Bichon's archival (ingest) timestamp.
    pub ingest_before: Option<i64>,
    pub account_ids: Option<HashSet<u64>>,
    pub mailbox_ids: Option<HashSet<u64>>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub message_id: Option<String>,
    pub has_attachment: Option<bool>,
    pub attachment_name: Option<String>,
    /// Matches if the message carries at least one of the given facet tags
    /// (OR semantics). An empty set is ignored.
    pub tags: Option<HashSet<String>>,
    /// Tri-state tag-presence filter, mirroring `has_attachment`:
    /// `Some(true)` keeps only tagged messages, `Some(false)` keeps only
    /// messages with no tag at all, and `None` (the default) does not filter.
    ///
    /// A tag filter cannot express "untagged" — the absence of a value is not
    /// a value — so this is a separate dimension rather than a special tag
    /// name. `Some(false)` combined with a non-empty `tags` is a
    /// contradiction and is rejected by [`EmailSearchRequest::validate`].
    pub has_tags: Option<bool>,
    pub attachment_extension: Option<String>,
    pub attachment_category: Option<String>,
    pub attachment_content_type: Option<String>,
}

impl EmailSearchFilter {
    /// Whether the filter carries a usable tag selection. An empty set is
    /// treated as "no tag filter" by the query builder, so it does not
    /// contradict `has_tags = Some(false)`.
    fn has_tag_selection(&self) -> bool {
        self.tags.as_ref().is_some_and(|tags| !tags.is_empty())
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum SortBy {
    #[default]
    DATE,
    SIZE,
    /// Sort by the IMAP server INTERNALDATE timestamp.
    #[serde(rename = "INTERNAL_DATE")]
    #[cfg_attr(feature = "web-api", oai(rename = "INTERNAL_DATE"))]
    InternalDate,
    /// Sort by Bichon's archival (ingest) timestamp.
    #[serde(rename = "INGEST_AT")]
    #[cfg_attr(feature = "web-api", oai(rename = "INGEST_AT"))]
    IngestAt,
    /// Sort by full-text relevance (BM25), Tantivy's own scoring. Only
    /// meaningful when the query carries a text term; always descending (a
    /// higher score = more relevant), so the `desc` flag is ignored.
    #[serde(rename = "RELEVANCE")]
    #[cfg_attr(feature = "web-api", oai(rename = "RELEVANCE"))]
    Relevance,
}

/// Resolve the effective sort order. The API default is DATE; when the query
/// carries a full-text term we default to RELEVANCE instead — a date-desc
/// default buries the best match under the most recent mail, which is the
/// opposite of what a user searching expects. An explicitly requested sort is
/// always respected, except an explicit RELEVANCE with no text term: there is
/// no score to order by, so it falls back to DATE.
fn resolve_sort_by(requested: Option<SortBy>, has_text: bool) -> SortBy {
    match requested {
        Some(SortBy::Relevance) if !has_text => SortBy::DATE,
        Some(s) => s,
        None if has_text => SortBy::Relevance,
        None => SortBy::DATE,
    }
}

fn has_text_filter(filter: &EmailSearchFilter) -> bool {
    filter.text.is_some() || filter.subject.is_some() || filter.body.is_some()
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct EmailSearchRequest {
    pub filter: EmailSearchFilter,
    pub page: u64,
    pub page_size: u64,
    pub sort_by: Option<SortBy>,
    pub desc: Option<bool>,
}
impl EmailSearchRequest {
    pub fn validate(&self) -> BichonResult<()> {
        if self.page == 0 || self.page_size == 0 {
            return Err(raise_error!(
                "Both page and page_size must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.page_size > 500 {
            return Err(raise_error!(
                "The page_size exceeds the maximum allowed limit of 500.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        // "untagged" and "has one of these tags" cannot both hold, so the
        // combination is a caller mistake rather than an empty result the
        // user would have to interpret.
        if self.filter.has_tags == Some(false) && self.filter.has_tag_selection() {
            return Err(raise_error!(
                "has_tags=false cannot be combined with tags: an untagged message matches none of the selected tags.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        Ok(())
    }
}

pub fn search_messages_impl(
    accounts: Option<HashSet<u64>>,
    request: EmailSearchRequest,
) -> BichonResult<DataPage<Envelope>> {
    request.validate()?;
    let sort_by = resolve_sort_by(request.sort_by, has_text_filter(&request.filter));
    ENVELOPE_MANAGER.search(
        accounts,
        request.filter,
        request.page,
        request.page_size,
        request.desc.unwrap_or(true),
        sort_by,
    )
}

/// Search with a pre-resolved scope (account + per-source filter groups).
///
/// Used by the REST layer after `crate::account::access::search_scope`, which
/// expands view accounts into their sources with the view's filters. The
/// request's own `account_ids` are ignored: the scope already encodes them,
/// and a view id would otherwise be applied as a second constraint that
/// matches no document.
pub fn search_messages_scoped(
    scope: Option<Vec<crate::account::access::ViewScopeGroup>>,
    request: EmailSearchRequest,
) -> BichonResult<DataPage<Envelope>> {
    request.validate()?;
    let sort_by = resolve_sort_by(request.sort_by, has_text_filter(&request.filter));
    ENVELOPE_MANAGER.search_scoped(
        scope,
        request.filter,
        request.page,
        request.page_size,
        request.desc.unwrap_or(true),
        sort_by,
    )
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AttachmentSearchFilter {
    pub id: Option<String>,
    pub text: Option<String>,
    /// See [`EmailSearchFilter::match_all`].
    #[serde(default)]
    pub match_all: Option<bool>,
    pub subject: Option<String>,
    pub from: Option<String>,
    pub since: Option<i64>,
    pub before: Option<i64>,
    pub account_ids: Option<HashSet<u64>>,
    pub mailbox_ids: Option<HashSet<u64>>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,

    pub attachment_name: Option<String>,
    pub content_hash: Option<String>,

    pub tags: Option<HashSet<String>>,
    pub attachment_extension: Option<String>,
    pub attachment_category: Option<String>,
    pub attachment_content_type: Option<String>,

    pub is_ocr: Option<bool>,
    pub is_message: Option<bool>,
    pub has_text: Option<bool>,

    pub min_page_count: Option<u64>,
    pub max_page_count: Option<u64>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AttachmentSearchRequest {
    filter: AttachmentSearchFilter,
    page: u64,
    page_size: u64,
    sort_by: Option<SortBy>,
    desc: Option<bool>,
}
impl AttachmentSearchRequest {
    pub fn filter(&self) -> &AttachmentSearchFilter {
        &self.filter
    }

    pub fn validate(&self) -> BichonResult<()> {
        if self.page == 0 || self.page_size == 0 {
            return Err(raise_error!(
                "Both page and page_size must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.page_size > 500 {
            return Err(raise_error!(
                "The page_size exceeds the maximum allowed limit of 500.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        Ok(())
    }
}

pub fn search_attachment_impl(
    accounts: Option<HashSet<u64>>,
    request: AttachmentSearchRequest,
) -> BichonResult<DataPage<AttachmentModel>> {
    request.validate()?;
    let has_text = request.filter().text.is_some() || request.filter().subject.is_some();
    let sort_by = resolve_sort_by(request.sort_by, has_text);
    ATTACHMENT_MANAGER.search(
        accounts,
        request.filter,
        request.page,
        request.page_size,
        request.desc.unwrap_or(true),
        sort_by,
    )
}

/// Attachment search with a scope resolved from view accounts. See
/// `AttachmentManager::search_scoped` for why filtered sources are excluded.
pub fn search_attachment_scoped(
    scope: Option<Vec<crate::account::access::ViewScopeGroup>>,
    request: AttachmentSearchRequest,
) -> BichonResult<DataPage<AttachmentModel>> {
    request.validate()?;
    let has_text = request.filter().text.is_some() || request.filter().subject.is_some();
    let sort_by = resolve_sort_by(request.sort_by, has_text);
    ATTACHMENT_MANAGER.search_scoped(
        scope,
        request.filter,
        request.page,
        request.page_size,
        request.desc.unwrap_or(true),
        sort_by,
    )
}

/// The contact addresses a caller can see, without display names.
///
/// This address-only view is what the MCP tool exposes; the REST suggestion
/// endpoint uses [`contact_suggestions_in_scope`] instead. Both share
/// `contacts_by_address`, so the scope rules below cannot drift apart.
///
/// Views are expanded, because a view owns no documents: aggregating by the
/// caller's account ids alone returned an empty list to every viewer.
///
/// * An **unfiltered** source is aggregated straight from the index.
/// * A **filtered** source is sampled instead (the most recent matching
///   messages): the filter is part of the authorization boundary, so addresses
///   that appear only in messages the view does not expose must not be
///   suggested. This is a suggestion list, not a completeness claim — which is
///   why sampling is acceptable here and would not be in a search result.
pub fn contacts_in_scope(context: &ClientContext) -> BichonResult<HashSet<String>> {
    Ok(contacts_by_address(context)?.into_keys().collect())
}

/// The search UI's contact suggestions: every visible address paired with the
/// sender display name the archive recorded for it, sorted by address so the
/// picker's order is stable between calls.
pub fn contact_suggestions_in_scope(context: &ClientContext) -> BichonResult<Vec<Contact>> {
    let mut contacts: Vec<Contact> = contacts_by_address(context)?
        .into_iter()
        .map(|(email, name)| Contact { email, name })
        .collect();
    contacts.sort_by(|a, b| a.email.to_lowercase().cmp(&b.email.to_lowercase()));
    Ok(contacts)
}

/// Every visible address with its display name, when one is known.
///
/// Only senders carry a name (`from_name`); recipients contribute a bare
/// address. The same address seen as a recipient in one message and as a
/// sender in another keeps the name — see [`merge_contact`].
fn contacts_by_address(
    context: &ClientContext,
) -> BichonResult<HashMap<String, Option<String>>> {
    let Some(groups) = crate::account::access::search_scope(context, None)? else {
        // Unrestricted reader: every account.
        return ENVELOPE_MANAGER.get_all_contacts(None);
    };

    let mut contacts: HashMap<String, Option<String>> = HashMap::new();

    let unbounded: HashSet<u64> = groups
        .iter()
        .filter(|group| group.is_unfiltered())
        .map(|group| group.account_id)
        .collect();
    if !unbounded.is_empty() {
        for (address, name) in ENVELOPE_MANAGER.get_all_contacts(Some(unbounded))? {
            merge_contact(&mut contacts, address, name);
        }
    }

    const CONTACT_SAMPLE: u64 = 2_000;
    for group in groups.iter().filter(|group| !group.is_unfiltered()) {
        let page = ENVELOPE_MANAGER.search_scoped(
            Some(vec![group.clone()]),
            EmailSearchFilter::default(),
            1,
            CONTACT_SAMPLE,
            true,
            SortBy::DATE,
        )?;
        for envelope in page.items {
            collect_contacts(&envelope, &mut contacts);
        }
    }

    Ok(contacts)
}

/// Collect every address that appears in an envelope's participant headers.
fn collect_contacts(envelope: &Envelope, out: &mut HashMap<String, Option<String>>) {
    merge_contact(out, envelope.from.clone(), envelope.from_name.clone());
    for address in envelope
        .to
        .iter()
        .chain(envelope.cc.iter())
        .chain(envelope.bcc.iter())
    {
        merge_contact(out, address.clone(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(text: Option<&str>, subject: Option<&str>, body: Option<&str>) -> EmailSearchFilter {
        EmailSearchFilter {
            text: text.map(str::to_string),
            subject: subject.map(str::to_string),
            body: body.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn text_present_defaults_to_relevance() {
        let f = filter(Some("bienvenue"), None, None);
        assert!(has_text_filter(&f));
        assert_eq!(resolve_sort_by(None, has_text_filter(&f)), SortBy::Relevance);
    }

    #[test]
    fn subject_or_body_also_default_to_relevance() {
        let f = filter(None, Some("kroatien"), Some("skipper"));
        assert!(has_text_filter(&f));
        assert_eq!(resolve_sort_by(None, has_text_filter(&f)), SortBy::Relevance);
    }

    #[test]
    fn no_text_defaults_to_date() {
        let f = filter(None, None, None);
        assert!(!has_text_filter(&f));
        assert_eq!(resolve_sort_by(None, has_text_filter(&f)), SortBy::DATE);
    }

    #[test]
    fn explicit_date_is_not_overridden_by_text() {
        let f = filter(Some("bienvenue"), None, None);
        assert_eq!(
            resolve_sort_by(Some(SortBy::DATE), has_text_filter(&f)),
            SortBy::DATE
        );
    }

    #[test]
    fn explicit_relevance_with_text_is_kept() {
        let f = filter(Some("bienvenue"), None, None);
        assert_eq!(
            resolve_sort_by(Some(SortBy::Relevance), has_text_filter(&f)),
            SortBy::Relevance
        );
    }

    #[test]
    fn explicit_relevance_without_text_falls_back_to_date() {
        let f = filter(None, None, None);
        assert_eq!(
            resolve_sort_by(Some(SortBy::Relevance), has_text_filter(&f)),
            SortBy::DATE
        );
    }

    #[test]
    fn attachment_has_text_uses_text_or_subject() {
        // `search_attachment_impl` derives has_text from text/subject only.
        let with_text = AttachmentSearchFilter {
            text: Some("invoice".to_string()),
            ..Default::default()
        };
        let with_subject = AttachmentSearchFilter {
            subject: Some("invoice".to_string()),
            ..Default::default()
        };
        let no_text = AttachmentSearchFilter::default();
        assert!(with_text.text.is_some() || with_text.subject.is_some());
        assert!(with_subject.text.is_some() || with_subject.subject.is_some());
        assert!(no_text.text.is_none() && no_text.subject.is_none());
    }

    fn email_request(filter: EmailSearchFilter) -> EmailSearchRequest {
        EmailSearchRequest {
            filter,
            page: 1,
            page_size: 20,
            sort_by: None,
            desc: None,
        }
    }

    #[test]
    fn has_tags_alone_validates() {
        for has_tags in [true, false] {
            let request = email_request(EmailSearchFilter {
                has_tags: Some(has_tags),
                ..Default::default()
            });
            assert!(
                request.validate().is_ok(),
                "has_tags={has_tags} must be a valid filter on its own"
            );
        }
    }

    #[test]
    fn untagged_combined_with_tags_is_rejected() {
        let request = email_request(EmailSearchFilter {
            has_tags: Some(false),
            tags: Some(HashSet::from(["important".to_string()])),
            ..Default::default()
        });
        let err = request.validate().unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidParameter);
    }

    #[test]
    fn tagged_combined_with_tags_is_allowed() {
        let request = email_request(EmailSearchFilter {
            has_tags: Some(true),
            tags: Some(HashSet::from(["important".to_string()])),
            ..Default::default()
        });
        assert!(request.validate().is_ok());
    }

    #[test]
    fn untagged_with_an_empty_tag_set_is_allowed() {
        // An empty `tags` set is ignored by the query builder, so it does not
        // contradict `has_tags=false`.
        let request = email_request(EmailSearchFilter {
            has_tags: Some(false),
            tags: Some(HashSet::new()),
            ..Default::default()
        });
        assert!(request.validate().is_ok());
    }
}

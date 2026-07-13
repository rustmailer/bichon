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

use deunicode::deunicode;
use tantivy::query::{BooleanQuery, Occur, Query, RegexPhraseQuery, RegexQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Term};

use crate::error::{code::ErrorCode, BichonResult};
use crate::raise_error;

/// A field to search against wildcard-aware free text, together with whether
/// that field is analyzed by the "euro" tokenizer (lowercased/stemmed) or
/// indexed as a raw/exact `STRING` field.
#[derive(Clone, Copy)]
pub struct WildcardField {
    pub field: Field,
    pub analyzed: bool,
}

/// Returns `true` if `text` contains glob wildcard characters (`*` or `?`)
/// and therefore needs [`build_wildcard_query`] instead of tantivy's
/// `QueryParser`, which does not support wildcard term matching.
pub fn is_wildcard_query(text: &str) -> bool {
    text.contains('*') || text.contains('?')
}

/// Wraps fields that are analyzed by the "euro" tokenizer for wildcard search.
pub fn analyzed(fields: &[Field]) -> Vec<WildcardField> {
    fields
        .iter()
        .map(|&field| WildcardField {
            field,
            analyzed: true,
        })
        .collect()
}

/// Splits `word` into sub-tokens the same way tantivy's `SimpleTokenizer`
/// splits indexed text: on every non-alphanumeric boundary. `*`/`?` are kept
/// attached to whichever sub-token they border, so wildcard patterns survive
/// the same splitting the indexer applies at index time (e.g. `*.pdf` becomes
/// `["*", "pdf"]`, matching how `invoice.pdf` is indexed as two terms).
fn tokenize_word(word: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        if ch.is_alphanumeric() || ch == '*' || ch == '?' {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn normalize(token: &str, analyzed: bool) -> String {
    if analyzed {
        deunicode(&token.to_lowercase())
    } else {
        token.to_string()
    }
}

/// Converts a glob pattern (`*` = any run of characters, `?` = any single
/// character) into a regex pattern, escaping literal runs in one pass.
fn glob_to_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() + 4);
    let mut literal = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' | '?' => {
                if !literal.is_empty() {
                    regex.push_str(&regex::escape(&literal));
                    literal.clear();
                }
                regex.push_str(if ch == '*' { ".*" } else { "." });
            }
            _ => literal.push(ch),
        }
    }
    if !literal.is_empty() {
        regex.push_str(&regex::escape(&literal));
    }
    regex
}

fn token_query(field: Field, token: &str, analyzed: bool) -> BichonResult<Box<dyn Query>> {
    let normalized = normalize(token, analyzed);
    if token.contains('*') || token.contains('?') {
        let pattern = glob_to_regex(&normalized);
        Ok(Box::new(
            RegexQuery::from_pattern(&pattern, field)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?,
        ))
    } else {
        Ok(Box::new(TermQuery::new(
            Term::from_field_text(field, &normalized),
            IndexRecordOption::Basic,
        )))
    }
}

/// Builds a query for a single whitespace-separated `word` against one field.
///
/// Raw/exact fields are not tokenized at index time, so `word` is matched as
/// one term. Analyzed fields, however, are split into terms on punctuation by
/// the "euro" tokenizer's `SimpleTokenizer`; a `word` that spans a punctuation
/// boundary (e.g. `*@example.com`, `invoice*.pdf`) is split the same way and
/// matched as an adjacent phrase, so it lines up with how the field's content
/// was actually indexed.
fn field_query(wf: &WildcardField, word: &str) -> BichonResult<Option<Box<dyn Query>>> {
    if !wf.analyzed {
        return token_query(wf.field, word, false).map(Some);
    }

    let tokens = tokenize_word(word);
    match tokens.len() {
        0 => Ok(None),
        1 => token_query(wf.field, &tokens[0], true).map(Some),
        _ => {
            let patterns = tokens
                .iter()
                .map(|t| glob_to_regex(&normalize(t, true)))
                .collect();
            Ok(Some(Box::new(RegexPhraseQuery::new(wf.field, patterns))))
        }
    }
}

/// Builds a query over free text that may contain `*`/`?` glob wildcards.
///
/// Each whitespace-separated word is matched against every given field
/// (`Occur::Should`, mirroring tantivy `QueryParser`'s default OR semantics
/// across default fields), and words are themselves combined with
/// `Occur::Should`.
///
/// Analyzed fields are lowercased and ASCII-folded to line up with how the
/// "euro" tokenizer normalizes indexed terms; stemming, however, cannot be
/// replicated for a wildcard pattern, so matches are approximate against
/// stemmed forms (e.g. `run*` matches the stemmed term `run` but may miss
/// unstemmed variants). This also means plain (non-wildcard) words lose
/// stemming whenever they share a query string with a wildcard word, since
/// the whole string is routed through this wildcard-aware path instead of
/// tantivy's `QueryParser`.
pub fn build_wildcard_query(fields: &[WildcardField], text: &str) -> BichonResult<Box<dyn Query>> {
    let mut word_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

    for word in text.split_whitespace() {
        let mut field_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for wf in fields {
            if let Some(query) = field_query(wf, word)? {
                field_queries.push((Occur::Should, query));
            }
        }
        if !field_queries.is_empty() {
            word_queries.push((Occur::Should, Box::new(BooleanQuery::new(field_queries))));
        }
    }

    Ok(Box::new(BooleanQuery::new(word_queries)))
}

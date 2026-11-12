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

use mail_parser::HeaderForm;
use regex::{Captures, Regex};

fn merge_contiguous_encoded_words(input: &str) -> String {
    let block_re =
        Regex::new(r"(?:=\?[^?]+\?[bBqQ]\?[^?]+\?=)(?:\s+(?:=\?[^?]+\?[bBqQ]\?[^?]+\?=))+")
            .unwrap();

    let word_re = Regex::new(r"=\?([^?]+)\?([bBqQ])\?([^?]+)\?=").unwrap();

    block_re
        .replace_all(input, |caps: &Captures| {
            let whole = caps.get(0).unwrap().as_str();

            let mut charset: Option<String> = None;
            let mut encoding: Option<String> = None;
            let mut combined = String::new();
            let mut ok = true;

            for cap in word_re.captures_iter(whole) {
                let cs = &cap[1];
                let enc = cap[2].to_ascii_uppercase();
                let text = &cap[3];

                if let Some(ref c) = charset {
                    if c != cs {
                        ok = false;
                        break;
                    }
                } else {
                    charset = Some(cs.to_string());
                }

                if let Some(ref e) = encoding {
                    if e != &enc {
                        ok = false;
                        break;
                    }
                } else {
                    encoding = Some(enc);
                }

                combined.push_str(text);
            }

            if ok {
                format!(
                    "=?{}?{}?{}?=",
                    charset.unwrap(),
                    encoding.unwrap(),
                    combined
                )
            } else {
                whole.to_string()
            }
        })
        .to_string()
}

pub fn normalize_subject(raw_subject: Option<&str>) -> String {
    let subject = match raw_subject {
        Some(subject) => merge_contiguous_encoded_words(subject),
        None => return String::new(),
    };

    HeaderForm::Text
        .parse(subject.as_bytes())
        .value()
        .as_text()
        .map(String::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::envelope::utils::{merge_contiguous_encoded_words, normalize_subject};
    use mail_parser::MessageParser;

    // ── merge_contiguous_encoded_words ──────────────────────────────

    #[test]
    fn merge_basic_utf8_b() {
        let s = "Hello =?UTF-8?B?SGVsbG8=?= =?UTF-8?B?V29ybGQ=?= !!!";
        assert_eq!(
            merge_contiguous_encoded_words(s),
            "Hello =?UTF-8?B?SGVsbG8=V29ybGQ=?= !!!"
        );
    }

    #[test]
    fn merge_three_blocks() {
        let s = "=?UTF-8?B?QQ==?= =?UTF-8?B?Qg==?= =?UTF-8?B?Qw==?=";
        assert_eq!(
            merge_contiguous_encoded_words(s),
            "=?UTF-8?B?QQ==Qg==Qw==?="
        );
    }

    #[test]
    fn merge_noncontiguous_blocks() {
        let s = "=?UTF-8?B?QQ==?= =?UTF-8?B?Qg==?= test =?UTF-8?B?Qw==?= =?UTF-8?B?RA==?=";
        assert_eq!(
            merge_contiguous_encoded_words(s),
            "=?UTF-8?B?QQ==Qg==?= test =?UTF-8?B?Qw==RA==?="
        );
    }

    #[test]
    fn reject_different_charsets() {
        let s = "=?UTF-8?B?QQ==?= =?GBK?B?Qg==?=";
        assert_eq!(merge_contiguous_encoded_words(s), s);
    }

    #[test]
    fn reject_different_encodings() {
        let s = "=?UTF-8?B?QQ==?= =?UTF-8?Q?Qg?=";
        assert_eq!(merge_contiguous_encoded_words(s), s);
    }

    #[test]
    fn merge_case_insensitive_encoding() {
        let s = "=?UTF-8?b?QQ==?= =?UTF-8?B?Qg==?=";
        assert_eq!(merge_contiguous_encoded_words(s), "=?UTF-8?B?QQ==Qg==?=");
    }

    #[test]
    fn single_encoded_word_unchanged() {
        let s = "Hello =?UTF-8?B?SGVsbG8=?= !!!";
        assert_eq!(merge_contiguous_encoded_words(s), s);
    }

    #[test]
    fn multiple_spaces_between_words() {
        let s = "=?UTF-8?B?QQ==?=    =?UTF-8?B?Qg==?=";
        assert_eq!(merge_contiguous_encoded_words(s), "=?UTF-8?B?QQ==Qg==?=");
    }

    #[test]
    fn plain_subject_line() {
        let s = "Just a normal subject line";
        assert_eq!(merge_contiguous_encoded_words(s), s);
    }

    #[test]
    fn merge_quoted_printable() {
        let s = "=?UTF-8?Q?Hello_?= =?UTF-8?Q?World?=";
        assert_eq!(
            merge_contiguous_encoded_words(s),
            "=?UTF-8?Q?Hello_World?="
        );
    }

    // ── normalize_subject ───────────────────────────────────────────

    #[test]
    fn normalize_subject_none() {
        assert_eq!(normalize_subject(None), "");
    }

    /// The raw-subject fallback parses the header bytes after
    /// `merge_contiguous_encoded_words`; folded encoded words must survive
    /// that path (it runs when the decoded subject contains U+FFFD).
    #[test]
    fn normalize_subject_decodes_folded_encoded_words() {
        assert_eq!(
            normalize_subject(Some(" =?UTF-8?B?5L2g?=\r\n =?UTF-8?B?5aW9?=\r\n")),
            "你好"
        );
    }

    #[test]
    fn cp1253_subject_and_base64_body() {
        let eml = concat!(
            "Subject: =?Cp1253?B?1/Hd+fPnIOvv4+Hx6eHz7O/9?=\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: text/plain; charset=\"Cp1253\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "wePh8Of03SDw5evc9Oc=\r\n",
        );

        let message = MessageParser::default().parse(eml).unwrap();
        assert_eq!(message.subject(), Some("Χρέωση λογαριασμού"));
        assert_eq!(
            crate::envelope::extractor::first_body_text(&message).as_deref(),
            Some("Αγαπητέ πελάτη")
        );
    }

    /// Regression for #406: multibyte CJK charsets are only decodable when
    /// mail-parser is built with its `full_encoding` feature. Without it,
    /// `big5` labels decode as UTF-8 and produce U+FFFD.
    #[test]
    fn big5_subject_and_base64_body() {
        let eml = concat!(
            "Subject: =?big5?B?2HA=?=\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: text/plain; charset=\"big5\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "2HDYcNhwqrrYcNhw2HA=\r\n",
        );

        let message = MessageParser::default().parse(eml).unwrap();
        assert_eq!(message.subject(), Some("喵"));
        assert_eq!(
            crate::envelope::extractor::first_body_text(&message).as_deref(),
            Some("喵喵喵的喵喵喵")
        );
    }
}

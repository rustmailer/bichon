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


use std::collections::HashSet;
use std::sync::LazyLock;

/// Gmail system labels that describe message *state* rather than a folder.
/// They must never become the import destination.
///
/// Google localizes these system labels in Takeout's `X-Gmail-Labels` header
/// (e.g. French `Archivés`), so the English canonical tokens alone miss every
/// non-English account and the status label is wrongly treated as a business
/// folder. We therefore match the English tokens plus the known translations.
///
/// Best-effort from Gmail's UI label names; the reporter's real French export
/// confirmed `Archivés`/`Envoyé`. Extend or correct entries as real exports
/// confirm — only ever add *system* labels here, never user-created ones
/// (those are never localized).
static STATUS_LABELS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // English (canonical Takeout tokens)
        "Opened", "Unread", "Archived",
        // French
        "Ouverts", "Non lus", "Archivés",
        // Spanish
        "Abiertos", "No leídos", "Archivados",
        // Italian
        "Aperti", "Non letti", "Archiviati",
        // Portuguese
        "Abertos", "Não lidos", "Arquivados",
        // German
        "Geöffnet", "Ungelesen", "Archiviert",
        // Dutch
        "Geopend", "Ongelezen", "Gearchiveerd",
        // Swedish
        "Öppnade", "Olästa", "Arkiverade",
        // Danish
        "Åbnet", "Ulæst", "Arkiveret",
        // Norwegian
        "Åpnet", "Ulest", "Arkivert",
        // Finnish
        "Avatut", "Lukemattomat", "Arkistoidut",
        // Polish
        "Otworzone", "Nieprzeczytane", "Zarchiwizowane",
        // Czech
        "Otevřené", "Nepřečtené", "Archivováno",
        // Hungarian
        "Megnyitott", "Olvasatlan", "Archivált",
        // Romanian
        "Deschise", "Necitite", "Arhivate",
        // Russian
        "Открытые", "Непрочитанные", "В архиве",
        // Ukrainian
        "Відкриті", "Непрочитані", "В архіві",
        // Turkish
        "Açıldı", "Okunmamış", "Arşivlendi",
        // Greek
        "Ανοιγμένα", "Μη αναγνωσμένα", "Αρχειοθέτηση",
    ]
    .into_iter()
    .collect()
});

/// Gmail's generic location folders: real folders, but never a "business"
/// destination. Used only to prefer a custom label over them when several
/// labels remain.
static GENERIC_LOCATION_LABELS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // English
        "Inbox", "Sent",
        // French
        "Boîte de réception", "Envoyé",
        // Spanish
        "Recibidos", "Enviados",
        // Italian
        "Posta in arrivo", "Inviati",
        // Portuguese
        "Caixa de entrada", "Enviados",
        // German
        "Posteingang", "Gesendet",
        // Dutch
        "Postvak IN", "Verzonden items",
        // Swedish
        "Inkorgen", "Skickade",
        // Danish
        "Indbakke", "Sendt",
        // Norwegian
        "Innboks", "Sendt",
        // Finnish
        "Saapuneet", "Lähetetyt",
        // Polish
        "Odebrane", "Wysłane",
        // Czech
        "Doručená pošta", "Odeslané",
        // Hungarian
        "Beérkezett üzenetek", "Elküldött üzenetek",
        // Romanian
        "Căsuța de intrare", "Trimise",
        // Russian
        "Входящие", "Отправленные",
        // Ukrainian
        "Вхідні", "Надіслані",
        // Turkish
        "Gelen Kutusu", "Gönderilmiş",
        // Greek
        "Εισερχόμενα", "Απεσταλμένα",
    ]
    .into_iter()
    .collect()
});

pub fn determine_folder(labels_raw: &str) -> String {
    let all_labels: Vec<&str> = labels_raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if all_labels.is_empty() {
        return "Unknown".to_string();
    }

    let filtered: Vec<&str> = all_labels
        .iter()
        .filter(|&&l| !STATUS_LABELS.contains(l))
        .cloned()
        .collect();

    match filtered.len() {
        // Case A: If all labels were status labels, fallback to the first original label
        0 => all_labels[0].to_string(),
        // Case B: If only one label remains, that's our target destination
        1 => filtered[0].to_string(),
        // Case C: Multiple labels remain (e.g., ["Inbox", "medium"])
        _ => {
            // Prioritize custom business labels by excluding generic locations like "Inbox" or "Sent"
            let business_label = filtered
                .iter()
                .find(|&&l| !GENERIC_LOCATION_LABELS.contains(l));

            match business_label {
                // Return the first non-generic label found
                Some(label) => label.to_string(),
                // If only generic labels remain (e.g., ["Sent", "Inbox"]), pick the first available
                None => filtered[0].to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mail_parser::{HeaderForm, MessageParser};

    use super::*;

    fn parse_x_gmail_labels(raw_message: &[u8]) -> Option<String> {
        // Unknown headers are kept raw by default (no RFC 2047 decoding), so
        // parse them as unstructured text to decode `X-Gmail-Labels`. The
        // alternative is registering the name explicitly:
        // `.header("X-Gmail-Labels", HeaderForm::Text)`.
        let message = MessageParser::new()
            .unknown_headers(HeaderForm::Text)
            .parse(raw_message)?;
        message
            .headers()
            .value("X-Gmail-Labels")?
            .as_text()
            .map(|s| s.to_string())
    }

    /// Construct a raw MIME message with RFC 2047 encoded X-Gmail-Labels,
    /// parse it, and verify the header is correctly decoded.
    fn build_email(x_gmail_labels: &str) -> Vec<u8> {
        format!(
            "From: sender@example.com\r\n\
             To: recipient@example.com\r\n\
             Subject: Test\r\n\
             X-Gmail-Labels: {}\r\n\
             \r\n\
             Body text here.\r\n",
            x_gmail_labels
        )
        .into_bytes()
    }

    #[test]
    fn rfc2047_encoded_labels_are_decoded() {
        // Exactly the format the user reported: French Gmail labels
        let raw = build_email("=?UTF-8?Q?Corbeille?=, =?UTF-8?Q?Messages_archiv=C3=A9s?=");
        let labels = parse_x_gmail_labels(&raw).expect("failed to parse X-Gmail-Labels");

        // mail-parser decodes RFC 2047 header values during initial parsing.
        // The decoded text should NOT contain raw =?UTF-8?Q?... sequences.
        assert!(!labels.contains("=?UTF-8"), "labels still encoded: {labels:?}");
        assert!(labels.contains("Corbeille"), "missing 'Corbeille': {labels:?}");
        assert!(
            labels.contains("archivés"),
            "missing decoded 'archivés': {labels:?}",
        );

        // Full pipeline: decoded labels → determine_folder
        let folder = determine_folder(&labels);
        assert_eq!(folder, "Corbeille");
    }

    #[test]
    fn plain_ascii_labels_passthrough() {
        let raw = build_email("Inbox, Important");
        let labels = parse_x_gmail_labels(&raw).expect("failed to parse X-Gmail-Labels");
        assert_eq!(labels, "Inbox, Important");
        assert_eq!(determine_folder(&labels), "Important");
    }

    #[test]
    fn missing_x_gmail_labels_header() {
        let raw = b"From: sender@example.com\r\nTo: r@example.com\r\n\r\nBody.\r\n";
        let message = MessageParser::new().parse(raw.as_slice()).unwrap();
        assert!(message.headers().get("X-Gmail-Labels").is_none());
    }

    /// Issue: non-English system labels were treated as business folders.
    /// French "Archivés,Envoyé" must file under "Envoyé", like English
    /// "Archived,Sent" files under "Sent".
    #[test]
    fn french_archived_sent_goes_to_sent() {
        assert_eq!(determine_folder("Archivés,Envoyé"), "Envoyé");
        assert_eq!(determine_folder("Archived,Sent"), "Sent");
    }

    #[test]
    fn french_inbox_prefers_custom_label() {
        assert_eq!(determine_folder("Boîte de réception,Ma boîte"), "Ma boîte");
    }

    #[test]
    fn spanish_and_german_system_labels() {
        assert_eq!(determine_folder("Archivados,Enviados"), "Enviados");
        assert_eq!(determine_folder("Archiviert,Gesendet"), "Gesendet");
    }

    #[test]
    fn archived_only_falls_back_to_original_label() {
        // Same fallback as English: a message whose only label is the archive
        // status cannot be filed anywhere meaningful.
        assert_eq!(determine_folder("Archivés"), "Archivés");
        assert_eq!(determine_folder("Archived"), "Archived");
    }
}

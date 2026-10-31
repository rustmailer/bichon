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


use regex::Regex;
use std::panic;
use std::sync::LazyLock;
use tracing::error;

/// Removes remote content references from HTML email body.
///
/// Strips attributes that load content from http:// or https:// URLs,
/// keeping data: URIs and cid: references intact. Does NOT affect
/// navigation links (<a href>).
pub fn block_remote_content(html: &str) -> String {
    let mut result = html.to_string();

    // 1. Strip src, poster, data attributes with remote URLs.
    //    These always load content regardless of the tag.
    static SRC_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\s+(src|poster|data)\s*=\s*["'][^"']*(?:https?://|//)[^"']*["']"#).unwrap()
    });
    result = SRC_ATTR_RE.replace_all(&result, "").to_string();

    // 2. Strip srcset attributes with remote URLs.
    static SRCSET_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\s+srcset\s*=\s*["'][^"']*(?:https?://|//)[^"']*["']"#).unwrap()
    });
    result = SRCSET_ATTR_RE.replace_all(&result, "").to_string();

    // 3. Strip href on <link> tags (stylesheets), never <a> links.
    static LINK_HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(<link\b[^>]*)\s+href\s*=\s*["'][^"']*(?:https?://|//)[^"']*["']"#).unwrap()
    });
    result = LINK_HREF_RE.replace_all(&result, "$1").to_string();

    // 4. Strip CSS url() references with remote URLs in inline styles.
    static CSS_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)url\(\s*["']?\s*(?:https?://|//)[^)"'\s]*\s*["']?\s*\)"#).unwrap()
    });
    result = CSS_URL_RE.replace_all(&result, "").to_string();

    // 5. Strip @import url(...) with remote URLs inside <style> blocks.
    static IMPORT_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)@import\s+url\(\s*["']?\s*(?:https?://|//)[^)"'\s]*\s*["']?\s*\)\s*;"#,
        )
        .unwrap()
    });
    result = IMPORT_URL_RE.replace_all(&result, "").to_string();

    // 6. Strip background attribute on <body> with remote URLs.
    static BODY_BG_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(<body\b[^>]*)\s+background\s*=\s*["'][^"']*(?:https?://|//)[^"']*["']"#)
            .unwrap()
    });
    result = BODY_BG_RE.replace_all(&result, "$1").to_string();

    result
}

/// Convert an HTML body to plain text.
///
/// Takes `&str` rather than an owned `String`: callers hold the body as a
/// `Cow` borrowed from the raw message, and an owned signature forced a full
/// copy of the HTML just to convert it — a large one on newsletters.
pub fn extract_text(html: &str) -> String {
    let result = panic::catch_unwind(|| {
        html2text::config::plain()
            .allow_width_overflow()
            .string_from_read(html.as_bytes(), 100)
    });

    match result {
        Ok(Ok(text)) => text,
        Ok(Err(err)) => {
            error!("html2text error: {}", err);
            html.to_string()
        }
        Err(err) => {
            if let Some(s) = err.downcast_ref::<&str>() {
                error!("html2text panic: {}", s);
            } else if let Some(s) = err.downcast_ref::<String>() {
                error!("html2text panic: {}", s);
            } else {
                error!("html2text panic: unknown error");
            }
            html.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_from_plain_html() {
        let html = "<html><body><p>Hello World</p></body></html>".to_string();
        let text = extract_text(&html);
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn extract_text_strips_tags() {
        let html = "<div><h1>Title</h1><p>Paragraph with <b>bold</b> text.</p></div>".to_string();
        let text = extract_text(&html);
        assert!(text.contains("Title"));
        assert!(text.contains("Paragraph"));
        assert!(text.contains("bold"));
        assert!(!text.contains("<h1>"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn extract_text_empty_string() {
        let html = "".to_string();
        let text = extract_text(&html);
        assert!(text.is_empty());
    }

    #[test]
    fn extract_text_plain_text_passthrough() {
        let html = "Just some plain text without any HTML tags.".to_string();
        let text = extract_text(&html);
        assert!(text.contains("plain text"));
    }

    #[test]
    fn extract_text_with_links() {
        let html = "<a href=\"https://example.com\">Click here</a>".to_string();
        let text = extract_text(&html);
        assert!(text.contains("Click here"));
    }

    mod block_remote {
        use super::*;

        #[test]
        fn strips_img_src_http() {
            let html = r#"<img src="https://tracker.example.com/pixel.gif" alt="x">"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://tracker.example.com"));
            assert!(result.contains("alt=")); // other attrs preserved
        }

        #[test]
        fn strips_img_src_protocol_relative() {
            let html = r#"<img src="//tracker.example.com/pixel.gif">"#;
            let result = block_remote_content(html);
            assert!(!result.contains("//tracker.example.com"));
        }

        #[test]
        fn preserves_data_uri() {
            let html = r#"<img src="data:image/png;base64,ABC123" alt="embedded">"#;
            let result = block_remote_content(html);
            assert!(result.contains("data:image/png;base64,ABC123"));
        }

        #[test]
        fn preserves_cid_reference() {
            let html = r#"<img src="cid:abc123@example.com" alt="inline">"#;
            let result = block_remote_content(html);
            assert!(result.contains("cid:abc123@example.com"));
        }

        #[test]
        fn preserves_anchor_href() {
            let html = r#"<a href="https://example.com/page">Click</a>"#;
            let result = block_remote_content(html);
            assert!(result.contains(r#"href="https://example.com/page""#));
        }

        #[test]
        fn strips_link_stylesheet_href() {
            let html =
                r#"<link rel="stylesheet" href="https://fonts.example.com/font.css">"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://fonts.example.com"));
            assert!(result.contains("<link")); // tag preserved
        }

        #[test]
        fn strips_script_src() {
            let html = r#"<script src="https://evil.example.com/malware.js"></script>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://evil.example.com"));
        }

        #[test]
        fn strips_iframe_src() {
            let html = r#"<iframe src="https://ads.example.com/banner"></iframe>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://ads.example.com"));
        }

        #[test]
        fn strips_css_url_in_style() {
            let html = r#"<div style="background: url(https://tracker.example.com/bg.jpg)"></div>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://tracker.example.com"));
        }

        #[test]
        fn strips_css_url_protocol_relative() {
            let html = r#"<div style="background: url(//tracker.example.com/bg.jpg)"></div>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("//tracker.example.com"));
        }

        #[test]
        fn strips_css_import() {
            let html =
                r#"<style>@import url("https://fonts.example.com/font.css");</style>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://fonts.example.com"));
        }

        #[test]
        fn strips_video_poster() {
            let html = r#"<video poster="https://cdn.example.com/thumb.jpg"></video>"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://cdn.example.com"));
        }

        #[test]
        fn strips_srcset() {
            let html =
                r#"<img srcset="https://cdn.example.com/img1.jpg 1x, https://cdn.example.com/img2.jpg 2x">"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://cdn.example.com"));
        }

        #[test]
        fn strips_body_background() {
            let html = r#"<body background="https://tracker.example.com/bg.jpg">"#;
            let result = block_remote_content(html);
            assert!(!result.contains("https://tracker.example.com"));
            assert!(result.contains("<body"));
        }

        #[test]
        fn handles_mixed_content() {
            let html = r#"
            <html>
              <body>
                <img src="https://spy.example.com/pixel.gif" width="1" height="1">
                <img src="data:image/png;base64,OK123" alt="ok">
                <a href="https://example.com/read-more">Read more</a>
                <div style="background: url(https://tracker.example.com/bg.jpg) no-repeat"></div>
              </body>
            </html>"#;
            let result = block_remote_content(html);
            // Remote content gone
            assert!(!result.contains("spy.example.com"));
            assert!(!result.contains("tracker.example.com"));
            // Safe content preserved
            assert!(result.contains("data:image/png;base64,OK123"));
            assert!(result.contains(r#"href="https://example.com/read-more""#));
        }
    }
}

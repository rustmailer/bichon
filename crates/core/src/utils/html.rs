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


use lol_html::{element, rewrite_str, RewriteStrSettings};
use regex::Regex;
use std::collections::HashSet;
use std::panic;
use std::sync::LazyLock;
use tracing::error;

/// Tags that are removed together with everything inside them. Leaving their
/// contents in the document would either render as text (`<script>`) or be
/// parsed as markup (`<template>`), so the tag and its subtree both go.
const DROP_WITH_CONTENT: &[&str] = &[
    "applet", "base", "embed", "frame", "frameset", "iframe", "link", "math", "noembed",
    "noframes", "object", "plaintext", "script", "svg", "template", "xmp",
];

/// Tags kept as-is. This is what an email actually needs to render; the list
/// deliberately excludes forms, media, frames and anything that embeds a
/// document of its own.
const ALLOWED_TAGS: &[&str] = &[
    "a", "abbr", "acronym", "address", "article", "aside", "b", "bdi", "bdo", "big",
    "blockquote", "body", "br", "caption", "center", "cite", "code", "col", "colgroup",
    "data", "dd", "del", "details", "dfn", "div", "dl", "dt", "em", "figcaption", "figure",
    "font", "footer", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr",
    "html", "i", "img", "ins", "kbd", "li", "main", "mark", "nav", "ol", "p", "pre", "q",
    "rp", "rt", "rtc", "ruby", "s", "samp", "section", "small", "span", "strike", "strong",
    "style", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "time",
    "title", "tr", "tt", "u", "ul", "var", "wbr",
];

/// Attributes allowed on any kept tag. They affect presentation only.
const GENERIC_ATTRS: &[&str] = &[
    "align", "bgcolor", "border", "cellpadding", "cellspacing", "char", "charoff", "class",
    "color", "dir", "face", "height", "hspace", "id", "lang", "nowrap", "size", "style",
    "title", "valign", "vspace", "width",
];

/// Attributes allowed only on specific tags. `meta` and URL-valued attributes
/// are handled separately in `sanitize_email_html`.
fn tag_attrs(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "name"],
        "blockquote" | "q" | "del" | "ins" => &["cite", "datetime"],
        "body" | "table" | "tbody" | "tfoot" | "thead" | "tr" => &["background", "summary"],
        "col" | "colgroup" => &["span"],
        "details" => &["open"],
        "img" => &["alt", "src"],
        "li" => &["type", "value"],
        "ol" => &["reversed", "start", "type"],
        "style" => &["media", "type"],
        "td" | "th" => &[
            "abbr", "background", "colspan", "headers", "rowspan", "scope", "summary",
        ],
        "time" => &["datetime"],
        "ul" => &["type"],
        _ => &[],
    }
}

static ALLOWED_TAG_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ALLOWED_TAGS.iter().copied().collect());
static GENERIC_ATTR_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| GENERIC_ATTRS.iter().copied().collect());

/// Decode the character references a browser would decode in an attribute
/// value. lol_html deliberately hands us the raw value, so `&#106;avascript:`
/// or `javascript&colon;` must be normalised before the scheme check.
fn decode_char_refs(value: &str) -> String {
    fn entity(name: &str) -> Option<char> {
        if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
        if let Some(dec) = name.strip_prefix('#') {
            return dec.parse::<u32>().ok().and_then(char::from_u32);
        }
        match name.to_ascii_lowercase().as_str() {
            "colon" => Some(':'),
            "tab" => Some('\t'),
            "newline" => Some('\n'),
            "sol" => Some('/'),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => None,
        }
    }

    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let decoded = after
            .find(';')
            .and_then(|semi| entity(&after[..semi]).map(|ch| (ch, semi + 1)));
        match decoded {
            Some((ch, consumed)) => {
                out.push(ch);
                rest = &after[consumed..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// URL-valued attributes we keep: which schemes may appear in each.
fn url_scheme_allowed(attribute: &str, value: &str) -> bool {
    // Browsers ignore control characters and whitespace inside a scheme
    // (`java\tscript:` is still javascript:), so validate the compacted form.
    let decoded = decode_char_refs(value);
    let compact: String = decoded
        .chars()
        .filter(|c| !c.is_ascii_control() && !c.is_ascii_whitespace())
        .collect();
    let lower = compact.to_ascii_lowercase();

    let scheme_end = match lower.find(':') {
        Some(i) => i,
        // No scheme: a relative URL or a fragment. Keep it.
        None => return true,
    };
    let scheme = &lower[..scheme_end];
    if scheme.contains('/') {
        // The colon is part of a path, not a scheme.
        return true;
    }

    match (attribute, scheme) {
        ("href", "http" | "https" | "mailto" | "tel") => true,
        ("cite" | "background", "http" | "https") => true,
        ("src", "http" | "https") => true,
        // Inline images: collect_attachments already rewrote cid: to data:.
        // `data:image/svg+xml` cannot run script when loaded through <img>.
        ("src", "data") => lower.starts_with("data:image/"),
        ("src", "cid") => true,
        _ => false,
    }
}

fn is_url_attribute(name: &str) -> bool {
    matches!(name, "href" | "src" | "cite" | "background")
}

/// Server-side allowlist sanitiser for HTML email bodies.
///
/// The WebUI renders mail with `srcdoc` inside a sandboxed iframe and adds a
/// CSP meta tag, but those defences live in the client. Sanitising here means
/// every API consumer receives HTML that is safe to render on its own: tags,
/// attributes and URL schemes outside the allowlist are dropped, and anything
/// new is dropped by default rather than needing a blacklist entry.
///
/// Layout fidelity matters — mail is full of tables, inline styles and
/// `<style>` blocks — so those are preserved. CSS cannot execute script in a
/// sandboxed document without `allow-scripts`, and the CSP injected by the
/// WebUI already blocks remote style/image loads until the user opts in.
pub fn sanitize_email_html(html: &str) -> String {
    let settings = RewriteStrSettings::new().append_element_content_handler(element!(
        "*",
        |el| {
            let tag = el.tag_name();

                if tag == "meta" {
                    if el.get_attribute("http-equiv").is_some() {
                        // <meta http-equiv=refresh> is a navigation primitive.
                        el.remove();
                        return Ok(());
                    }
                } else if DROP_WITH_CONTENT.contains(&tag.as_str()) {
                    el.remove();
                    return Ok(());
                } else if !ALLOWED_TAG_SET.contains(tag.as_str()) {
                    // Unknown/unwanted wrappers (forms, media, custom
                    // elements): drop the tag but keep the visible content.
                    el.remove_and_keep_content();
                    return Ok(());
                }

                let mut to_remove: Vec<String> = Vec::new();
                let allowed_here: HashSet<&str> = if tag == "meta" {
                    ["charset", "name", "content"].into_iter().collect()
                } else {
                    tag_attrs(&tag).iter().copied().collect()
                };
                for attr in el.attributes() {
                    let name = attr.name();
                    let lower = name.to_ascii_lowercase();
                    let allowed = !lower.starts_with("on")
                        && (GENERIC_ATTR_SET.contains(lower.as_str())
                            || allowed_here.contains(lower.as_str()));
                    if !allowed
                        || (is_url_attribute(&lower)
                            && !url_scheme_allowed(&lower, &attr.value()))
                    {
                        to_remove.push(name);
                    }
                }
                for name in to_remove {
                    el.remove_attribute(&name);
                }
            Ok(())
        }
    ));
    let result = rewrite_str(html, settings);

    match result {
        Ok(sanitized) => sanitized,
        Err(err) => {
            // Never fail open: an unparseable body is shown as escaped text
            // rather than handed to the browser as raw HTML.
            error!("email HTML sanitisation failed: {err}");
            html.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        }
    }
}

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

    mod sanitize {
        use super::*;

        #[test]
        fn script_is_removed_with_its_content() {
            let out = sanitize_email_html("<p>a</p><script>alert(1)</script><p>b</p>");
            assert!(!out.to_lowercase().contains("<script"));
            assert!(!out.contains("alert(1)"));
            assert!(out.contains('a') && out.contains('b'));
        }

        #[test]
        fn event_handlers_and_javascript_urls_are_dropped() {
            let out = sanitize_email_html(
                r#"<img src="x" onerror="alert(1)"><a href="javascript:alert(1)">x</a>"#,
            );
            let lower = out.to_lowercase();
            assert!(!lower.contains("onerror"));
            assert!(!lower.contains("javascript:"));
            assert!(out.contains(">x</a>"));
        }

        #[test]
        fn encoded_javascript_schemes_are_dropped() {
            for html in [
                r#"<a href="&#106;avascript:alert(1)">x</a>"#,
                r#"<a href="javascript&colon;alert(1)">x</a>"#,
                "<a href=\"java&#x09;script:alert(1)\">x</a>",
            ] {
                let out = sanitize_email_html(html);
                assert!(
                    !out.contains("href"),
                    "encoded scheme survived: {out}"
                );
            }
        }

        #[test]
        fn layout_html_is_preserved() {
            let html = r##"<html><head><style>.a{color:red}</style></head><body><table width="600" cellpadding="0" bgcolor="#fff"><tr><td colspan="2" align="center" style="color:red" class="a">x</td></tr></table></body></html>"##;
            let out = sanitize_email_html(html);
            for needle in [
                "<style>",
                ".a{color:red}",
                "width=\"600\"",
                "cellpadding=\"0\"",
                "bgcolor=\"#fff\"",
                "colspan=\"2\"",
                "align=\"center\"",
                "style=\"color:red\"",
                "class=\"a\"",
            ] {
                assert!(out.contains(needle), "missing {needle}: {out}");
            }
        }

        #[test]
        fn inline_images_survive_and_other_data_urls_do_not() {
            let out = sanitize_email_html(
                r#"<img src="data:image/png;base64,AA"><img src="cid:abc@example.com"><a href="data:text/html,x">d</a>"#,
            );
            assert!(out.contains("data:image/png;base64,AA"));
            assert!(out.contains("cid:abc@example.com"));
            assert!(!out.contains("data:text/html"));
        }

        #[test]
        fn frames_objects_forms_and_meta_refresh_are_removed() {
            let out = sanitize_email_html(
                r#"<iframe src="x"></iframe><object data="x"></object><form action="x"><p>body</p></form><meta http-equiv="refresh" content="0;url=x"><meta charset="utf-8">"#,
            );
            let lower = out.to_lowercase();
            assert!(!lower.contains("<iframe"));
            assert!(!lower.contains("<object"));
            assert!(!lower.contains("<form"));
            assert!(!lower.contains("http-equiv"));
            assert!(lower.contains("<meta charset"));
            assert!(out.contains("body"));
        }

        #[test]
        fn srcset_and_unknown_attributes_are_dropped() {
            let out = sanitize_email_html(
                r#"<img src="https://e/x.png" srcset="https://e/2x.png 2x" data-foo="1" draggable="true">"#,
            );
            assert!(out.contains("src="));
            assert!(!out.contains("srcset"));
            assert!(!out.contains("data-foo"));
            assert!(!out.contains("draggable"));
        }

        #[test]
        fn relative_and_anchor_links_survive() {
            let out =
                sanitize_email_html(r##"<a href="#top">t</a><a href="/page">p</a>"##);
            assert!(out.contains("#top"));
            assert!(out.contains("/page"));
        }
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

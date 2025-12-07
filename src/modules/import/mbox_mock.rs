/// Mock mbox generator for comprehensive testing
///
/// This module generates realistic mbox files containing various email types:
/// - Plain text emails
/// - Multipart/alternative (HTML + text)
/// - Emails with attachments (images, PDFs, etc.)
/// - Nested EML attachments (emails attached to emails)
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

/// Generates a complete mbox file with diverse email examples
pub fn generate_comprehensive_mbox() -> String {
    let mut mbox = String::new();

    // 1. Plain text email
    mbox.push_str(&generate_plain_text_email());

    // 2. Multipart/alternative email (HTML + text)
    mbox.push_str(&generate_multipart_alternative_email());

    // 3. Email with PDF attachment
    mbox.push_str(&generate_email_with_pdf_attachment());

    // 4. Email with image attachment
    mbox.push_str(&generate_email_with_image_attachment());

    // 5. Email with multiple attachments
    mbox.push_str(&generate_email_with_multiple_attachments());

    // 6. Email with nested EML attachment
    mbox.push_str(&generate_email_with_eml_attachment());

    // 7. Complex nested: EML containing email with attachments
    mbox.push_str(&generate_complex_nested_eml());

    mbox
}

fn generate_plain_text_email() -> String {
    format!(
        "From MAILER-DAEMON Mon Dec 04 10:00:00 2025
From: alice@example.com
To: bob@example.com
Subject: Plain text test email
Date: Mon, 4 Dec 2025 10:00:00 -0500
Message-ID: <plain-001@example.com>
Content-Type: text/plain; charset=utf-8

This is a plain text email for testing keyword search.
Important keywords: quarterly report, financial analysis, Q4 results.

The meeting is scheduled for next Tuesday at 2pm.

Best regards,
Alice
"
    )
}

fn generate_multipart_alternative_email() -> String {
    format!(
        r#"From MAILER-DAEMON Mon Dec 04 11:00:00 2025
From: charlie@example.com
To: diana@example.com
Subject: Multipart HTML and text email
Date: Mon, 4 Dec 2025 11:00:00 -0500
Message-ID: <multipart-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="boundary-alt-12345"

--boundary-alt-12345
Content-Type: text/plain; charset=utf-8

This is the plain text version.
Project milestone: Phase 2 completion deadline is Friday.
Keywords: deployment, production release, staging environment.

--boundary-alt-12345
Content-Type: text/html; charset=utf-8

<html>
<body>
<h1>This is the HTML version</h1>
<p>Project milestone: <strong>Phase 2 completion</strong> deadline is Friday.</p>
<p>Keywords: <em>deployment</em>, <em>production release</em>, <em>staging environment</em>.</p>
</body>
</html>
--boundary-alt-12345--
"#
    )
}

fn generate_email_with_pdf_attachment() -> String {
    // Simple mock PDF content (PDF magic bytes + minimal structure)
    let pdf_content = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj 2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj 3 0 obj<</Type/Page/MediaBox[0 0 612 792]/Parent 2 0 R/Resources<<>>>>endobj\nxref\n0 4\ntrailer<</Size 4/Root 1 0 R>>\nstartxref\n%%EOF";
    let pdf_base64 = BASE64.encode(pdf_content);

    format!(
        r#"From MAILER-DAEMON Mon Dec 04 12:00:00 2025
From: frank@example.com
To: grace@example.com
Subject: Email with PDF attachment - Contract Review
Date: Mon, 4 Dec 2025 12:00:00 -0500
Message-ID: <pdf-attachment-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="boundary-mixed-pdf-67890"

--boundary-mixed-pdf-67890
Content-Type: text/plain; charset=utf-8

Please review the attached contract document.
Keywords: legal review, NDA agreement, vendor contract, compliance check.

The document contains sensitive information about the merger.

--boundary-mixed-pdf-67890
Content-Type: application/pdf; name="contract_2025.pdf"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="contract_2025.pdf"

{}
--boundary-mixed-pdf-67890--
"#,
        pdf_base64
    )
}

fn generate_email_with_image_attachment() -> String {
    // Minimal PNG (1x1 red pixel)
    let png_content = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
        0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
        0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
        0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let png_base64 = BASE64.encode(&png_content);

    format!(
        r#"From MAILER-DAEMON Mon Dec 04 13:00:00 2025
From: henry@example.com
To: iris@example.com
Subject: Marketing Campaign Screenshots
Date: Mon, 4 Dec 2025 13:00:00 -0500
Message-ID: <image-attachment-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="boundary-mixed-img-11111"

--boundary-mixed-img-11111
Content-Type: text/plain; charset=utf-8

Attached are the screenshots from the new marketing campaign.
Keywords: brand redesign, logo update, color palette, user interface mockup.

Please provide feedback by end of week.

--boundary-mixed-img-11111
Content-Type: image/png; name="campaign_mockup.png"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="campaign_mockup.png"

{}
--boundary-mixed-img-11111--
"#,
        png_base64
    )
}

fn generate_email_with_multiple_attachments() -> String {
    let txt_content = BASE64.encode(b"Meeting notes:\n- Budget approved\n- Timeline extended\n- New hires needed");
    let csv_content = BASE64.encode(b"Name,Department,Budget\nEngineering,5000000\nMarketing,2000000");

    format!(
        r#"From MAILER-DAEMON Mon Dec 04 14:00:00 2025
From: jack@example.com
To: kelly@example.com
Subject: Q4 Budget Report - Multiple Attachments
Date: Mon, 4 Dec 2025 14:00:00 -0500
Message-ID: <multi-attach-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="boundary-multi-22222"

--boundary-multi-22222
Content-Type: text/plain; charset=utf-8

Please find attached the Q4 budget report and meeting notes.
Keywords: financial planning, resource allocation, headcount increase, capital expenditure.

Let me know if you need any clarifications.

--boundary-multi-22222
Content-Type: text/plain; name="meeting_notes.txt"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="meeting_notes.txt"

{}
--boundary-multi-22222
Content-Type: text/csv; name="budget_breakdown.csv"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="budget_breakdown.csv"

{}
--boundary-multi-22222--
"#,
        txt_content, csv_content
    )
}

fn generate_email_with_eml_attachment() -> String {
    // Create a nested email to attach
    let nested_email = format!(
        r#"From: nested@example.com
To: recipient@example.com
Subject: Forwarded: Important nested message
Date: Mon, 4 Dec 2025 09:00:00 -0500
Message-ID: <nested-inner-001@example.com>
Content-Type: text/plain; charset=utf-8

This is a forwarded email with important information.
Keywords: patent application, intellectual property, trademark filing.

The deadline for submission is approaching.
"#
    );

    let eml_base64 = BASE64.encode(nested_email.as_bytes());

    format!(
        r#"From MAILER-DAEMON Mon Dec 04 15:00:00 2025
From: laura@example.com
To: mike@example.com
Subject: FWD: Please review this forwarded email
Date: Mon, 4 Dec 2025 15:00:00 -0500
Message-ID: <eml-attachment-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="boundary-eml-33333"

--boundary-eml-33333
Content-Type: text/plain; charset=utf-8

Please review the attached forwarded email about the patent application.
Keywords: legal documentation, prior art search, patent claims.

--boundary-eml-33333
Content-Type: message/rfc822; name="forwarded_message.eml"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="forwarded_message.eml"

{}
--boundary-eml-33333--
"#,
        eml_base64
    )
}

fn generate_complex_nested_eml() -> String {
    // Create an email with an attachment (innermost)
    let innermost_email = format!(
        r#"From: deepnest@example.com
To: recipient@example.com
Subject: Original email with attachment
Date: Mon, 4 Dec 2025 08:00:00 -0500
Message-ID: <deepnest-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="inner-boundary-44444"

--inner-boundary-44444
Content-Type: text/plain; charset=utf-8

This is the original email deep in the nesting.
Keywords: source code repository, git commit history, code review feedback.

--inner-boundary-44444
Content-Type: text/plain; name="source_notes.txt"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="source_notes.txt"

{}
--inner-boundary-44444--
"#,
        BASE64.encode(b"Source code notes:\n- Refactor authentication module\n- Add unit tests\n- Update documentation")
    );

    let eml_base64 = BASE64.encode(innermost_email.as_bytes());

    format!(
        r#"From MAILER-DAEMON Mon Dec 04 16:00:00 2025
From: nancy@example.com
To: oliver@example.com
Subject: FWD: FWD: Complex nested forwarding chain
Date: Mon, 4 Dec 2025 16:00:00 -0500
Message-ID: <complex-nested-001@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="boundary-complex-55555"

--boundary-complex-55555
Content-Type: text/plain; charset=utf-8

This email contains a forwarded email which itself contains attachments.
Keywords: software architecture, microservices design, API documentation, database schema.

This tests our ability to extract content from deeply nested structures.

--boundary-complex-55555
Content-Type: message/rfc822; name="original_with_attachment.eml"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="original_with_attachment.eml"

{}
--boundary-complex-55555--
"#,
        eml_base64
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_comprehensive_mbox() {
        let mbox = generate_comprehensive_mbox();

        // Verify it contains all expected email types
        assert!(mbox.contains("Plain text test email"));
        assert!(mbox.contains("Multipart HTML and text email"));
        assert!(mbox.contains("PDF attachment"));
        assert!(mbox.contains("Marketing Campaign Screenshots"));
        assert!(mbox.contains("Multiple Attachments"));
        assert!(mbox.contains("forwarded_message.eml"));
        assert!(mbox.contains("Complex nested forwarding"));

        // Verify keywords are present
        assert!(mbox.contains("quarterly report"));
        assert!(mbox.contains("deployment"));
        assert!(mbox.contains("legal review"));
        assert!(mbox.contains("brand redesign"));
        assert!(mbox.contains("financial planning"));
        assert!(mbox.contains("patent application"));
        assert!(mbox.contains("microservices design"));
    }
}

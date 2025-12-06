use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

use crate::modules::account::migration::{AccountModel, AccountType};
use crate::modules::error::{BichonResult, code::ErrorCode};
use crate::modules::import::mbox::import_mbox_from_path;
use crate::modules::import::mbox_mock::generate_comprehensive_mbox;
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use crate::modules::message::content::retrieve_email_content;
use crate::raise_error;

macro_rules! search {
    ($searcher:expr, $query:expr, $limit:expr) => {
        $searcher.search($query.as_ref(), &tantivy::collector::TopDocs::with_limit($limit))
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))
    };
}

macro_rules! get_doc {
    ($searcher:expr, $addr:expr) => {
        $searcher.doc($addr)
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))
    };
}

#[tokio::test]
async fn test_comprehensive_mbox_import_and_search() -> BichonResult<()> {
    // Setup test environment
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    std::env::set_var("BICHON_DATA_DIR", &data_dir);

    // Generate comprehensive mbox file
    let mbox_content = generate_comprehensive_mbox();
    let mbox_path = temp_dir.path().join("comprehensive_test.mbox");
    let mut file = File::create(&mbox_path)?;
    file.write_all(mbox_content.as_bytes())?;

    // Create test account
    let account = AccountModel {
        id: 1,
        email: "test@example.com".to_string(),
        account_type: AccountType::NoSync,
        ..Default::default()
    };
    account.save().await?;

    // Import the mbox file
    import_mbox_from_path(&mbox_path, account.id, "comprehensive_test".to_string()).await?;

    // Allow indexer to process
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Search for messages
    let searcher = ENVELOPE_INDEX_MANAGER.create_searcher()?;
    let query = ENVELOPE_INDEX_MANAGER.account_query(account.id);
    let top_docs = search!(searcher, query, 20)?;

    println!("Found {} messages", top_docs.len());
    assert_eq!(top_docs.len(), 7, "Expected 7 messages in mbox file");

    // Test 1: Verify plain text email is searchable
    let plain_text_query = ENVELOPE_INDEX_MANAGER.text_query("quarterly report");
    let plain_results = search!(searcher, plain_text_query, 10)?;
    assert!(!plain_results.is_empty(), "Should find 'quarterly report' in plain text email");

    // Test 2: Verify multipart email text content is indexed
    let multipart_query = ENVELOPE_INDEX_MANAGER.text_query("deployment");
    let multipart_results = search!(searcher, multipart_query, 10)?;
    assert!(!multipart_results.is_empty(), "Should find 'deployment' in multipart email");

    // Test 3: Verify email with PDF attachment metadata
    let pdf_query = ENVELOPE_INDEX_MANAGER.text_query("legal review");
    let pdf_results = search!(searcher, pdf_query, 10)?;
    assert!(!pdf_results.is_empty(), "Should find 'legal review' in PDF attachment email");

    // Test 4: Verify email with image attachment
    let image_query = ENVELOPE_INDEX_MANAGER.text_query("brand redesign");
    let image_results = search!(searcher, image_query, 10)?;
    assert!(!image_results.is_empty(), "Should find 'brand redesign' in image attachment email");

    // Test 5: Verify multiple attachments email
    let multi_query = ENVELOPE_INDEX_MANAGER.text_query("financial planning");
    let multi_results = search!(searcher, multi_query, 10)?;
    assert!(!multi_results.is_empty(), "Should find 'financial planning' in multi-attachment email");

    // Test 6: Verify nested EML attachment
    let nested_query = ENVELOPE_INDEX_MANAGER.text_query("patent application");
    let nested_results = search!(searcher, nested_query, 10)?;
    assert!(!nested_results.is_empty(), "Should find 'patent application' in nested EML");

    // Test 7: Verify complex nested structure
    let complex_query = ENVELOPE_INDEX_MANAGER.text_query("microservices design");
    let complex_results = search!(searcher, complex_query, 10)?;
    assert!(!complex_results.is_empty(), "Should find 'microservices design' in complex nested email");

    // Test 8: Verify subject line search
    let subject_query = ENVELOPE_INDEX_MANAGER.subject_query("Contract Review");
    let subject_results = search!(searcher, subject_query, 10)?;
    assert!(!subject_results.is_empty(), "Should find email by subject");

    // Test 9: Verify sender search
    let from_query = ENVELOPE_INDEX_MANAGER.from_query("alice@example.com");
    let from_results = search!(searcher, from_query, 10)?;
    assert!(!from_results.is_empty(), "Should find email by sender");

    // Test 10: Verify attachment detection
    let attachment_query = ENVELOPE_INDEX_MANAGER.has_attachment_query(true);
    let attachment_results = search!(searcher, attachment_query, 10)?;
    println!("Found {} emails with attachments", attachment_results.len());
    assert!(attachment_results.len() >= 5, "Should find multiple emails with attachments");

    // Test 11: Retrieve and verify full content of emails
    for (_score, doc_address) in top_docs.iter().take(3) {
        let doc = get_doc!(searcher, *doc_address)?;
        let envelope = crate::modules::indexer::envelope::Envelope::from_tantivy_doc(&doc).await?;

        println!("\n=== Email ID: {} ===", envelope.id);
        println!("Subject: {}", if envelope.subject.is_empty() { "(no subject)" } else { &envelope.subject });

        // Retrieve full content
        let content = retrieve_email_content(account.id, envelope.id).await?;

        if let Some(text) = &content.text {
            println!("Text content length: {} chars", text.len());
            assert!(!text.is_empty(), "Email should have text content");
        }

        if let Some(html) = &content.html {
            println!("HTML content length: {} chars", html.len());
        }

        if let Some(attachments) = &content.attachments {
            println!("Attachments: {}", attachments.len());
            for att in attachments {
                println!("  - {} ({}, {} bytes)",
                    att.filename,
                    att.file_type,
                    att.size
                );
            }
        }
    }

    // Test 12: Verify HTML content is indexed (from multipart/alternative)
    let html_keyword_query = ENVELOPE_INDEX_MANAGER.text_query("Phase 2 completion");
    let html_results = search!(searcher, html_keyword_query, 10)?;
    assert!(!html_results.is_empty(), "Should find keywords from HTML part of multipart email");

    // Test 13: Search across different fields
    let keywords_to_test = vec![
        ("Q4 results", "plain text"),
        ("production release", "multipart"),
        ("NDA agreement", "PDF attachment"),
        ("color palette", "image attachment"),
        ("resource allocation", "multiple attachments"),
        ("intellectual property", "nested EML"),
        ("API documentation", "complex nested"),
    ];

    for (keyword, description) in keywords_to_test {
        let keyword_query = ENVELOPE_INDEX_MANAGER.text_query(keyword);
        let results = search!(searcher, keyword_query, 10)?;
        assert!(!results.is_empty(),
            "Should find keyword '{}' from {} email", keyword, description);
    }

    println!("\n✓ All comprehensive mbox tests passed!");
    Ok(())
}

#[tokio::test]
async fn test_attachment_content_extraction() -> BichonResult<()> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    std::env::set_var("BICHON_DATA_DIR", &data_dir);

    let mbox_content = generate_comprehensive_mbox();
    let mbox_path = temp_dir.path().join("attachment_test.mbox");
    let mut file = File::create(&mbox_path)?;
    file.write_all(mbox_content.as_bytes())?;

    let account = AccountModel {
        id: 2,
        email: "attachment@example.com".to_string(),
        account_type: AccountType::NoSync,
        ..Default::default()
    };
    account.save().await?;

    import_mbox_from_path(&mbox_path, account.id, "attachment_test".to_string()).await?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let searcher = ENVELOPE_INDEX_MANAGER.create_searcher()?;
    let query = ENVELOPE_INDEX_MANAGER.account_query(account.id);
    let top_docs = search!(searcher, query, 20)?;

    let mut found_pdf = false;
    let mut found_png = false;
    let mut found_txt = false;
    let mut found_csv = false;
    let mut found_eml = false;

    for (_score, doc_address) in top_docs {
        let doc = get_doc!(searcher, doc_address)?;
        let envelope = crate::modules::indexer::envelope::Envelope::from_tantivy_doc(&doc).await?;
        let content = retrieve_email_content(account.id, envelope.id).await?;

        if let Some(attachments) = content.attachments {
            for att in attachments {
                match att.file_type.as_str() {
                    "application/pdf" => {
                        assert_eq!(att.filename, "contract_2025.pdf");
                        found_pdf = true;
                    }
                    "image/png" => {
                        assert_eq!(att.filename, "campaign_mockup.png");
                        found_png = true;
                    }
                    t if t.starts_with("text/plain") => {
                        if att.filename == "meeting_notes.txt" {
                            found_txt = true;
                        }
                    }
                    "text/csv" => {
                        assert_eq!(att.filename, "budget_breakdown.csv");
                        found_csv = true;
                    }
                    "message/rfc822" => {
                        found_eml = true;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(found_pdf, "Should find PDF attachment");
    assert!(found_png, "Should find PNG attachment");
    assert!(found_txt, "Should find TXT attachment");
    assert!(found_csv, "Should find CSV attachment");
    assert!(found_eml, "Should find EML attachment");

    println!("✓ All attachment types detected correctly!");
    Ok(())
}

#[tokio::test]
async fn test_nested_eml_content_extraction() -> BichonResult<()> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    std::env::set_var("BICHON_DATA_DIR", &data_dir);

    let mbox_content = generate_comprehensive_mbox();
    let mbox_path = temp_dir.path().join("nested_test.mbox");
    let mut file = File::create(&mbox_path)?;
    file.write_all(mbox_content.as_bytes())?;

    let account = AccountModel {
        id: 3,
        email: "nested@example.com".to_string(),
        account_type: AccountType::NoSync,
        ..Default::default()
    };
    account.save().await?;

    import_mbox_from_path(&mbox_path, account.id, "nested_test".to_string()).await?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Search for content that should only be in nested emails
    let searcher = ENVELOPE_INDEX_MANAGER.create_searcher()?;

    // These keywords are ONLY in the nested EML attachments
    let nested_only_keywords = vec![
        "patent claims",           // In simple nested EML
        "trademark filing",        // In simple nested EML
        "source code repository",  // In complex nested EML
        "git commit history",      // In complex nested EML
    ];

    for keyword in nested_only_keywords {
        let query = ENVELOPE_INDEX_MANAGER.text_query(keyword);
        let results = search!(searcher, query, 10)?;
        assert!(!results.is_empty(),
            "Should find keyword '{}' from nested EML content", keyword);
    }

    println!("✓ Nested EML content extraction working correctly!");
    Ok(())
}

// Temporary mock data for dashboard responsive testing
import { DashboardStats } from './api';

const NOW = Date.now();
const DAY = 86400000;

export const MOCK_DASHBOARD_STATS: DashboardStats = {
  account_count: 12,
  email_count: 145892,
  attachment_count: 34201,
  total_size_bytes: 128_849_018_880, // ~120 GB logical
  storage_usage_bytes: 85_899_345_920, // ~80 GB blob
  index_usage_bytes: 12_884_901_888, // ~12 GB index
  recent_activity: Array.from({ length: 30 }, (_, i) => ({
    timestamp_ms: NOW - (29 - i) * DAY,
    count: Math.floor(Math.random() * 2000) + 200,
  })),
  top_senders: [
    { key: 'alexander.hamilton@verylongemaildomain-truncation-test.com', count: 4523 },
    { key: 'noreply@github-enterprise-notifications.system.example.org', count: 3891 },
    { key: 'jane.doe+project-alpha-beta-gamma@company-with-long-name.io', count: 3102 },
    { key: 'newsletter-subscriptions@really-long-marketing-domain.co.uk', count: 2845 },
    { key: 'support-tickets+priority-high@helpdesk.corporate.example.com', count: 2100 },
    { key: 'bot-pipeline-ci-cd-failures@devops.internal.long-subdomain.net', count: 1789 },
    { key: 'short@s.dev', count: 1500 },
    { key: 'alerts-monitoring-productions-east-us@observability-platform.com', count: 1256 },
    { key: 'weekly-digest-no-reply@newsletter.huge-media-conglomerate.org', count: 980 },
    { key: 'invitations-events-calendar-reminders@social-network-app.io', count: 760 },
  ],
  top_accounts: [
    { key: 'primary.work.mailbox@enterprise-long-domain-name.com', count: 78500, account_id: 1 },
    // Two accounts sharing one address (import + live sync) must stay
    // distinguishable in the dashboard: same key, different account_id.
    { key: 'personal.archive+all@very-lengthy-personal-domain.me', count: 42300, account_id: 2 },
    { key: 'personal.archive+all@very-lengthy-personal-domain.me', count: 15100, account_id: 3 },
    { key: 'team-leads@department-of-engineering.corp.example.org', count: 8992, account_id: 4 },
    { key: 'short@x.co', count: 1000, account_id: 5 },
  ],
  with_attachment_count: 18500,
  without_attachment_count: 15701,
  top_largest_emails: [
    {
      id: 'msg-001',
      subject: 'RE: [EXTERNAL] Q4 Financial Reports & Budget Planning Documents for Review - Please Provide Feedback by EOD Friday with Department Head Sign-off Required',
      size_bytes: 52_428_800,
    },
    {
      id: 'msg-002',
      subject: 'Fwd: Urgent: Client Presentation Draft - Version 7 Final (With Legal Team Amendments and Compliance Review Attached)',
      size_bytes: 48_234_496,
    },
    {
      id: 'msg-003',
      subject: 'Meeting Minutes: Cross-Functional Architecture Review Session - Microservices Migration Strategy and Timeline Discussion (Part 3 of 5)',
      size_bytes: 41_943_040,
    },
    {
      id: 'msg-004',
      subject: 'Invoice #INV-2026-04582 - Professional Services Engagement: Cloud Infrastructure Assessment and Remediation Planning Phase II Deliverables',
      size_bytes: 38_797_312,
    },
    {
      id: 'msg-005',
      subject: '[ACTION REQUIRED] Security Incident Response: Post-Mortem Analysis and Remediation Steps for CVE-2026-12345 - Department-Wide Mandatory Review',
      size_bytes: 35_651_584,
    },
    {
      id: 'msg-006',
      subject: 'Monthly Newsletter: Engineering Blog Digest - Articles on Distributed Systems, Rust Async Runtime Internals, and Performance Optimization Techniques',
      size_bytes: 31_457_280,
    },
    {
      id: 'msg-007',
      subject: 'Contract Review: Master Service Agreement Amendment #7 with Third-Party Vendor Integration Services for Payment Processing Platform',
      size_bytes: 28_311_552,
    },
    {
      id: 'msg-008',
      subject: 'Travel Itinerary & Expense Report: International Conference on Systems Programming - Accommodation, Flight, and Per Diem Documentation Package',
      size_bytes: 25_165_824,
    },
    {
      id: 'msg-009',
      subject: 'Re: [INTERNAL] Employee Onboarding Documentation Package - Benefits Enrollment, Tax Forms, Direct Deposit Setup, and IT Access Request Forms Bundle',
      size_bytes: 22_020_096,
    },
    {
      id: 'msg-010',
      subject: 'Data Export Request: Complete Transaction History 2024-2026 with Audit Trail and Compliance Certification for External Regulatory Review Board',
      size_bytes: 18_874_368,
    },
  ],
  top_largest_attachments: [
    { id: 'att-001', name: 'Q4_2025_Financial_Statements_Audited_with_Supporting_Schedules_and_Notes_v3_FINAL.xlsx', size_bytes: 45_254_100 },
    { id: 'att-002', name: 'project_deliverables_package_phase_2_with_test_results_coverage_report_and_deployment_guide.zip', size_bytes: 38_900_500 },
    { id: 'att-003', name: '2026-01-15_production_database_backup_full_with_transaction_logs_and_stored_procedures.sql.gz', size_bytes: 35_200_000 },
    { id: 'att-004', name: 'client_presentation_deck_v7_final_approved_with_speaker_notes_and_embedded_video_demo.pptx', size_bytes: 31_000_000 },
    { id: 'att-005', name: 'system_architecture_diagrams_microservices_v2_with_sequence_flows_and_deployment_topology.pdf', size_bytes: 28_500_000 },
    { id: 'att-006', name: 'annual_company_event_photograph_high_resolution_group_photo_panorama_2026.jpg', size_bytes: 25_000_000 },
    { id: 'att-007', name: 'complete_source_code_archive_feature_branch_refactor_auth_module_2026_01_15.tar.gz', size_bytes: 22_800_000 },
    { id: 'att-008', name: 'product_demo_screencast_walkthrough_new_features_2026_release_candidate.mp4', size_bytes: 19_500_000 },
    { id: 'att-009', name: 'legal_contract_review_package_with_redlined_amendments_and_counsel_opinion_letters.pdf', size_bytes: 16_200_000 },
    { id: 'att-010', name: 'employee_training_module_compliance_and_security_awareness_2026_v2_interactive.iso', size_bytes: 12_800_000 },
  ],
  system_version: '1.0.1',
};

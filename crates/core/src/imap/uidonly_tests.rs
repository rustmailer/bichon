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

use super::uidonly::{exact_args, inventory_args, UidOnlyHandle, UidOnlyLimits, UidOnlyStream};
use futures::TryStreamExt;
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
struct ScriptIo {
    input: VecDeque<u8>,
    max_chunk: usize,
    pending_at_eof: bool,
    writes: Arc<Mutex<Vec<u8>>>,
}

impl ScriptIo {
    fn new(input: impl Into<Vec<u8>>, max_chunk: usize) -> (Self, Arc<Mutex<Vec<u8>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                input: input.into().into(),
                max_chunk,
                pending_at_eof: false,
                writes: Arc::clone(&writes),
            },
            writes,
        )
    }

    fn pending(input: impl Into<Vec<u8>>, max_chunk: usize) -> Self {
        let (mut io, _) = Self::new(input, max_chunk);
        io.pending_at_eof = true;
        io
    }
}

impl AsyncRead for ScriptIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        destination: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let count = destination
            .remaining()
            .min(self.max_chunk)
            .min(self.input.len());
        if count == 0 && self.pending_at_eof {
            return Poll::Pending;
        }
        for _ in 0..count {
            destination.put_slice(&[self.input.pop_front().expect("input length checked")]);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ScriptIo {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writes.lock().expect("writes poisoned").extend(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

type Session = async_imap::Session<UidOnlyStream<ScriptIo>>;

async fn authenticated(
    after_login: &[u8],
    limits: UidOnlyLimits,
    chunk: usize,
) -> (Session, UidOnlyHandle, Arc<Mutex<Vec<u8>>>) {
    let mut transcript = b"* OK synthetic ready\r\nA0001 OK LOGIN completed\r\n".to_vec();
    transcript.extend_from_slice(after_login);
    let (io, writes) = ScriptIo::new(transcript, chunk);
    let (stream, handle) = UidOnlyStream::new(io, limits).expect("valid limits");
    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .expect("greeting read")
        .expect("greeting present");
    let session = client.login("synthetic", "redacted").await.expect("login");
    (session, handle, writes)
}

async fn enabled(
    after_enable: &[u8],
    limits: UidOnlyLimits,
    chunk: usize,
) -> (Session, UidOnlyHandle) {
    let mut transcript = b"* ENABLED UIDONLY\r\nA0002 OK ENABLE completed\r\n".to_vec();
    transcript.extend_from_slice(after_enable);
    let (mut session, handle, _) = authenticated(&transcript, limits, chunk).await;
    session
        .run_command_and_check_ok("ENABLE UIDONLY")
        .await
        .expect("enable");
    handle.ensure_active().expect("adapter active");
    (session, handle)
}

async fn rejected_fetch(response: &[u8], exact: bool) {
    let (mut session, handle) = enabled(response, UidOnlyLimits::default(), 1).await;
    if exact {
        handle
            .arm_next_fetch_literal_limit(16)
            .expect("arm body bound");
    }
    let stream = if exact {
        session
            .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
            .await
    } else {
        session
            .uid_fetch("1:7", "(UID RFC822.SIZE) (PARTIAL 1:7)")
            .await
    }
    .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    assert!(handle.poison_reason().is_some());
}

#[tokio::test]
async fn public_async_imap_session_parses_fragmented_uidfetch_without_touching_literal() {
    let body = b"From: sender@example.invalid\r\n\r\n* 9 UIDFETCH {999}\r\nsynthetic";
    let mut response = format!(
        "* 2 EXISTS\r\n* OK [UNSEEN 1] optional sequence metadata\r\n* OK [UIDVALIDITY 7] epoch\r\n* OK [UIDNEXT 50] next\r\nA0003 OK [READ-ONLY] EXAMINE completed\r\n\
         * 3 EXISTS\r\n* OK [UIDNEXT 51] arrival\r\n* 50 UIDFETCH (FLAGS (\\Seen))\r\n\
         * 42 UIDFETCH (UID 42 RFC822.SIZE {} BODY[] {{{}}}\r\n",
        body.len(),
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response.extend_from_slice(b")\r\nA0004 OK FETCH completed\r\n");

    let (mut session, handle) = enabled(&response, UidOnlyLimits::default(), 1).await;
    let mailbox = session.examine("Synthetic").await.expect("examine");
    assert_eq!(mailbox.exists, 2);
    assert_eq!(mailbox.uid_validity, Some(7));
    assert_eq!(mailbox.uid_next, Some(50));

    let (set, query) = exact_args(42).expect("safe UID");
    handle
        .arm_next_fetch_literal_limit(body.len())
        .expect("arm body bound");
    let fetched: Vec<_> = session
        .uid_fetch(set, query)
        .await
        .expect("command")
        .try_collect()
        .await
        .expect("fetch response");
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].message, 42);
    assert_eq!(fetched[0].uid, Some(42));
    assert_eq!(fetched[0].body(), Some(body.as_slice()));
    assert_eq!(handle.literal_bytes_received(), body.len() as u64);
}

#[tokio::test]
async fn activation_requires_exact_enabled_and_matching_ok() {
    for response in [
        b"A0002 OK ENABLE completed\r\n".as_slice(),
        b"* ENABLED UIDONLY QRESYNC\r\nA0002 OK ENABLE completed\r\n".as_slice(),
        b"* ENABLED UIDONLY\r\nA9999 OK ENABLE completed\r\n".as_slice(),
        b"* ENABLED UIDONLY\r\nA0002 NO unavailable\r\n".as_slice(),
    ] {
        let (mut session, handle, _) = authenticated(response, UidOnlyLimits::default(), 2).await;
        assert!(session
            .run_command_and_check_ok("ENABLE UIDONLY")
            .await
            .is_err());
        assert!(handle.ensure_active().is_err());
        assert!(handle.poison_reason().is_some());
    }
}

#[tokio::test]
async fn forbidden_post_activation_responses_fail_closed() {
    for response in [
        b"* 7 FETCH (UID 7)\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* 7 EXPUNGE\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* VANISHED 7\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* OK [UNSEEN 7] sequence\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* OK [UIDNOTSTICKY] invalid\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* NO [MESSAGELIMIT 1000] bounded\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* SEARCH 7\r\nA0003 OK FETCH completed\r\n".as_slice(),
        b"* ESEARCH UID ALL 7\r\nA0003 OK FETCH completed\r\n".as_slice(),
    ] {
        rejected_fetch(response, true).await;
    }
}

#[tokio::test]
async fn body_must_be_full_and_literal_backed() {
    for response in [
        b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[] \"x\")\r\nA0003 OK FETCH completed\r\n"
            .as_slice(),
        b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[]<0> {1}\r\nx)\r\nA0003 OK FETCH completed\r\n"
            .as_slice(),
    ] {
        rejected_fetch(response, true).await;
    }
}

#[tokio::test]
async fn uidfetch_shape_accepts_requested_fields_in_either_order() {
    let response = b"* 42 UIDFETCH (RFC822.SIZE 1 UID 42)\r\nA0003 OK inventory\r\n\
                     * 42 UIDFETCH (RFC822.SIZE 1 UID 42 BODY[] {1}\r\nx)\r\nA0004 OK exact\r\n";
    let (mut session, handle) = enabled(response, UidOnlyLimits::default(), 1).await;
    let inventory: Vec<_> = session
        .uid_fetch("1:42", "(UID RFC822.SIZE) (PARTIAL 1:42)")
        .await
        .expect("inventory command")
        .try_collect()
        .await
        .expect("inventory response");
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].message, 42);
    assert_eq!(inventory[0].uid, Some(42));
    assert_eq!(inventory[0].size, Some(1));

    handle
        .arm_next_fetch_literal_limit(1)
        .expect("arm body bound");
    let exact: Vec<_> = session
        .uid_fetch("42", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("exact command")
        .try_collect()
        .await
        .expect("exact response");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].message, 42);
    assert_eq!(exact[0].uid, Some(42));
    assert_eq!(exact[0].size, Some(1));
    assert_eq!(exact[0].body(), Some(b"x".as_slice()));
}

#[tokio::test]
async fn uidfetch_shape_rejects_duplicates_mismatch_missing_and_extras() {
    let cases: &[(&[u8], bool)] = &[
        (
            b"* 7 UIDFETCH (UID 7 UID 7 RFC822.SIZE 1)\r\nA0003 OK done\r\n",
            false,
        ),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 RFC822.SIZE 1)\r\nA0003 OK done\r\n",
            false,
        ),
        (
            b"* 7 UIDFETCH (UID 8 RFC822.SIZE 1)\r\nA0003 OK done\r\n",
            false,
        ),
        (
            b"* 7 UIDFETCH (UID 7 FLAGS () RFC822.SIZE 1)\r\nA0003 OK done\r\n",
            false,
        ),
        (b"* 7 UIDFETCH (UID 7)\r\nA0003 OK done\r\n", false),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            false,
        ),
        (
            b"* 7 UIDFETCH (UID 7 UID 7 RFC822.SIZE 1 BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 RFC822.SIZE 1 BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 8 RFC822.SIZE 1 BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 7 BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 FLAGS () BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[] BODY[] {1}\r\nx)\r\nA0003 OK done\r\n",
            true,
        ),
        (
            b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[] {1}\r\nx FLAGS ())\r\nA0003 OK done\r\n",
            true,
        ),
    ];
    for (response, exact) in cases {
        rejected_fetch(response, *exact).await;
    }
}

#[tokio::test]
async fn tagged_no_is_not_mistaken_for_an_empty_success() {
    rejected_fetch(b"A0003 NO [LIMIT] synthetic failure\r\n", false).await;
}

#[tokio::test]
async fn zero_exists_and_recent_are_valid_fetch_notifications() {
    let (mut session, handle) = enabled(
        b"* 0 EXISTS\r\n* 0 RECENT\r\nA0003 OK FETCH completed\r\n",
        UidOnlyLimits::default(),
        1,
    )
    .await;
    let stream = session
        .uid_fetch("1:9", "(UID RFC822.SIZE) (PARTIAL 1:9)")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.unwrap().is_empty());
    handle.ensure_active().expect("adapter remains healthy");
}

#[tokio::test]
async fn literal_limits_and_second_literals_reject_before_excess_body_bytes() {
    let mut limits = UidOnlyLimits::default();
    limits.max_literal_bytes = 4;
    limits.max_response_bytes = 128;
    limits.max_command_literal_bytes = 5;
    limits.max_command_response_bytes = 512;

    let response =
        b"* 7 UIDFETCH (UID 7 RFC822.SIZE 1 BODY[] {6}\r\nabcdef)\r\nA0003 OK FETCH completed\r\n";
    let (mut session, handle) = enabled(response, limits.clone(), 1).await;
    handle
        .arm_next_fetch_literal_limit(5)
        .expect("arm body bound");
    let stream = session
        .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    assert_eq!(handle.literal_bytes_received(), 0);

    let response = b"* 7 UIDFETCH (UID 7 RFC822.SIZE 6 BODY[] {3}\r\nabc BODY[HEADER] {3}\r\ndef)\r\nA0003 OK FETCH completed\r\n";
    let (mut session, handle) = enabled(response, limits, 1).await;
    handle
        .arm_next_fetch_literal_limit(5)
        .expect("arm body bound");
    let stream = session
        .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    assert_eq!(handle.literal_bytes_received(), 3);
    assert!(handle
        .poison_reason()
        .expect("poison")
        .contains("only closing"));
}

#[tokio::test]
async fn command_runtime_is_bounded_while_the_server_is_silent() {
    let transcript = b"* OK synthetic ready\r\nA0001 OK LOGIN completed\r\n\
                       * ENABLED UIDONLY\r\nA0002 OK ENABLE completed\r\n";
    let mut limits = UidOnlyLimits::default();
    limits.max_command_runtime = Duration::from_millis(10);
    let (stream, handle) =
        UidOnlyStream::new(ScriptIo::pending(transcript, 2), limits).expect("adapter");
    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .expect("greeting")
        .expect("greeting");
    let mut session = client.login("synthetic", "redacted").await.expect("login");
    session
        .run_command_and_check_ok("ENABLE UIDONLY")
        .await
        .expect("enable");
    handle
        .arm_next_fetch_literal_limit(16)
        .expect("arm body bound");
    let stream = session
        .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("command");
    let result = tokio::time::timeout(Duration::from_secs(1), stream.try_collect::<Vec<_>>())
        .await
        .expect("adapter deadline fired");
    assert!(result.is_err());
    assert!(handle
        .poison_reason()
        .expect("poison")
        .contains("timed out"));
}

#[tokio::test]
async fn response_count_is_bounded() {
    let mut limits = UidOnlyLimits::default();
    limits.max_command_responses = 2;
    let response = b"* 1 UIDFETCH (UID 1 RFC822.SIZE 1)\r\n\
                     * 2 UIDFETCH (UID 2 RFC822.SIZE 1)\r\n\
                     A0003 OK FETCH completed\r\n";
    let (mut session, handle) = enabled(response, limits, 4).await;
    let stream = session
        .uid_fetch("1:2", "(UID RFC822.SIZE) (PARTIAL 1:2)")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    assert!(handle
        .poison_reason()
        .expect("poison")
        .contains("response count"));
}

#[tokio::test]
async fn control_line_and_whole_response_bytes_are_bounded() {
    let mut limits = UidOnlyLimits::default();
    limits.max_control_line_bytes = 64;
    let response = b"* 7 UIDFETCH (UID 7 RFC822.SIZE 123456789 FLAGS (\\Seen \\Answered \\Flagged) BODY[] {1}\r\nx)\r\nA0003 OK done\r\n";
    let (mut session, handle) = enabled(response, limits, 2).await;
    handle
        .arm_next_fetch_literal_limit(1)
        .expect("arm body bound");
    let stream = session
        .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    assert!(handle
        .poison_reason()
        .expect("poison")
        .contains("control line"));

    let mut limits = UidOnlyLimits::default();
    limits.max_literal_bytes = 4;
    limits.max_response_bytes = 36;
    limits.max_command_literal_bytes = 4;
    limits.max_command_response_bytes = 128;
    let response = b"* 7 UIDFETCH (UID 7 RFC822.SIZE 4 BODY[] {4}\r\nabcd)\r\nA0003 OK done\r\n";
    let (mut session, handle) = enabled(response, limits, 2).await;
    handle
        .arm_next_fetch_literal_limit(4)
        .expect("arm body bound");
    let stream = session
        .uid_fetch("7", "(UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("command");
    assert!(stream.try_collect::<Vec<_>>().await.is_err());
    let reason = handle.poison_reason().expect("poison");
    assert!(reason.contains("response"), "{reason}");
}

#[tokio::test]
async fn one_in_flight_and_read_only_command_allowlist_are_enforced_before_write() {
    let (mut session, handle) = enabled(b"", UidOnlyLimits::default(), 8).await;
    handle
        .arm_next_fetch_literal_limit(16)
        .expect("arm body bound");
    session
        .run_command("UID FETCH 1 (UID RFC822.SIZE BODY.PEEK[])")
        .await
        .expect("first command");
    assert!(session.run_command("LOGOUT").await.is_err());
    assert!(handle.poison_reason().is_some());

    let (mut session, handle) = enabled(b"", UidOnlyLimits::default(), 8).await;
    assert!(session
        .run_command("UID STORE 1 +FLAGS (\\Deleted)")
        .await
        .is_err());
    assert!(handle.poison_reason().is_some());
}

#[test]
fn numeric_query_builders_reject_invalid_bounds() {
    assert_eq!(
        inventory_args(10, 20, 100).expect("valid inventory"),
        (
            "10:20".to_string(),
            "(UID RFC822.SIZE) (PARTIAL 1:100)".to_string()
        )
    );
    assert!(inventory_args(0, 20, 100).is_err());
    assert!(inventory_args(20, 10, 100).is_err());
    assert!(inventory_args(10, 20, 0).is_err());
    assert!(exact_args(0).is_err());
}

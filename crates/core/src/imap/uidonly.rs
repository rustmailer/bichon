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

// The routing layer lands later in the stack and consumes this module's API.
#![allow(dead_code)]

//! Bounded RFC 9586 compatibility for the released `async-imap` parser.
//!
//! The adapter is installed around the final transport before `Client` is
//! constructed. It is transparent through authentication, observes the exact
//! `ENABLE UIDONLY` exchange, then translates only top-level `UIDFETCH` atoms
//! to `FETCH`. Literal bytes are never searched or rewritten.

use crate::imap::session::SessionStream;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

const CHUNK: usize = 8 * 1024;

/// Hard limits applied before bytes reach `imap-proto`.
#[derive(Clone, Debug)]
pub(crate) struct UidOnlyLimits {
    pub max_control_line_bytes: usize,
    pub max_literal_bytes: usize,
    pub max_response_bytes: usize,
    pub max_command_literal_bytes: usize,
    pub max_command_response_bytes: usize,
    pub max_command_responses: usize,
    pub max_command_runtime: Duration,
}

impl Default for UidOnlyLimits {
    fn default() -> Self {
        Self {
            max_control_line_bytes: 64 * 1024,
            max_literal_bytes: 25 * 1024 * 1024,
            max_response_bytes: 26 * 1024 * 1024,
            max_command_literal_bytes: 100 * 1024 * 1024,
            max_command_response_bytes: 128 * 1024 * 1024,
            max_command_responses: 2_048,
            max_command_runtime: Duration::from_secs(5 * 60),
        }
    }
}

impl UidOnlyLimits {
    fn validate(&self) -> io::Result<()> {
        if self.max_control_line_bytes < 2
            || self.max_literal_bytes == 0
            || self.max_response_bytes < self.max_literal_bytes
            || self.max_command_literal_bytes < self.max_literal_bytes
            || self.max_command_response_bytes < self.max_response_bytes
            || self.max_command_responses < 2
            || self.max_command_runtime.is_zero()
        {
            return Err(invalid("invalid UIDONLY limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct UidOnlyHandle {
    health: Arc<Mutex<Health>>,
}

impl UidOnlyHandle {
    pub fn ensure_active(&self) -> io::Result<()> {
        let health = self.health.lock().expect("UIDONLY health poisoned");
        if let Some(reason) = &health.poison {
            return Err(invalid(reason.clone()));
        }
        if !health.active {
            return Err(invalid("UIDONLY activation was not confirmed"));
        }
        Ok(())
    }

    pub fn poison_reason(&self) -> Option<String> {
        self.health
            .lock()
            .expect("UIDONLY health poisoned")
            .poison
            .clone()
    }

    pub fn literal_bytes_received(&self) -> u64 {
        self.health
            .lock()
            .expect("UIDONLY health poisoned")
            .literal_bytes
    }

    /// Sets the pre-read literal ceiling for the next exact-message fetch.
    pub fn arm_next_fetch_literal_limit(&self, limit: usize) -> io::Result<()> {
        let mut health = self.health.lock().expect("UIDONLY health poisoned");
        if limit == 0
            || !health.active
            || health.command_in_flight
            || health.next_literal_limit.is_some()
        {
            return Err(invalid(
                "cannot arm UIDONLY literal limit in the current state",
            ));
        }
        health.next_literal_limit = Some(limit);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Health {
    active: bool,
    poison: Option<String>,
    literal_bytes: u64,
    command_in_flight: bool,
    next_literal_limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    PassThrough,
    Enabling,
    Active,
    Poisoned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    Enable,
    Examine,
    Inventory,
    ExactFetch,
    Logout,
}

#[derive(Debug)]
struct PendingCommand {
    tag: Vec<u8>,
    kind: CommandKind,
    saw_enabled: bool,
    response_bytes: usize,
    response_count: usize,
    literal_bytes: usize,
    literal_limit: usize,
    deadline: Pin<Box<Sleep>>,
}

/// Literal-aware stream bridge. Use [`wrap`] for Bichon's erased transport.
#[derive(Debug)]
pub(crate) struct UidOnlyStream<T> {
    inner: T,
    limits: UidOnlyLimits,
    health: Arc<Mutex<Health>>,
    mode: Mode,
    pending: Option<PendingCommand>,
    input: VecDeque<u8>,
    output: VecDeque<u8>,
    control_line: Vec<u8>,
    command_line: Vec<u8>,
    outgoing: VecDeque<u8>,
    literal_remaining: usize,
    response_bytes: usize,
    response_uidfetch: bool,
    first_line: bool,
    in_response: bool,
    eof: bool,
}

impl<T> UidOnlyStream<T> {
    pub fn new(inner: T, limits: UidOnlyLimits) -> io::Result<(Self, UidOnlyHandle)> {
        limits.validate()?;
        let health = Arc::new(Mutex::new(Health::default()));
        let handle = UidOnlyHandle {
            health: Arc::clone(&health),
        };
        Ok((
            Self {
                inner,
                limits,
                health,
                mode: Mode::PassThrough,
                pending: None,
                input: VecDeque::new(),
                output: VecDeque::new(),
                control_line: Vec::new(),
                command_line: Vec::new(),
                outgoing: VecDeque::new(),
                literal_remaining: 0,
                response_bytes: 0,
                response_uidfetch: false,
                first_line: true,
                in_response: false,
                eof: false,
            },
            handle,
        ))
    }

    fn fail(&mut self, kind: io::ErrorKind, reason: impl Into<String>) -> io::Error {
        let reason = reason.into();
        self.mode = Mode::Poisoned;
        let mut health = self.health.lock().expect("UIDONLY health poisoned");
        health.active = false;
        health.command_in_flight = false;
        health.next_literal_limit = None;
        health.poison = Some(reason.clone());
        io::Error::new(kind, reason)
    }

    fn check_deadline(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let elapsed = self
            .pending
            .as_mut()
            .is_some_and(|pending| pending.deadline.as_mut().poll(cx).is_ready());
        if elapsed {
            return Err(self.fail(io::ErrorKind::TimedOut, "UIDONLY command timed out"));
        }
        Ok(())
    }

    fn start_command(&mut self, tag: &[u8], kind: CommandKind) -> io::Result<()> {
        if self.pending.is_some() {
            return Err(self.fail(
                io::ErrorKind::InvalidInput,
                "UIDONLY permits only one command in flight",
            ));
        }
        let literal_limit = if kind == CommandKind::ExactFetch {
            let limit = self
                .health
                .lock()
                .expect("UIDONLY health poisoned")
                .next_literal_limit
                .take();
            let Some(limit) = limit else {
                return Err(self.fail(
                    io::ErrorKind::InvalidInput,
                    "exact UIDONLY fetch was not armed with a literal limit",
                ));
            };
            limit.min(self.limits.max_command_literal_bytes)
        } else {
            0
        };
        self.health
            .lock()
            .expect("UIDONLY health poisoned")
            .command_in_flight = true;
        self.pending = Some(PendingCommand {
            tag: tag.to_vec(),
            kind,
            saw_enabled: false,
            response_bytes: 0,
            response_count: 0,
            literal_bytes: 0,
            literal_limit,
            deadline: Box::pin(tokio::time::sleep(self.limits.max_command_runtime)),
        });
        Ok(())
    }

    fn validate_command(&mut self, line: &[u8]) -> io::Result<()> {
        let line = line
            .strip_suffix(b"\r\n")
            .ok_or_else(|| invalid("outbound IMAP line is not CRLF terminated"))?;
        let Some(space) = line.iter().position(|byte| *byte == b' ') else {
            if self.mode == Mode::PassThrough {
                return Ok(());
            }
            return Err(self.fail(io::ErrorKind::InvalidInput, "untagged UIDONLY command"));
        };
        let (tag, command) = (&line[..space], &line[space + 1..]);
        if tag.is_empty() || tag == b"*" || tag == b"+" {
            if self.mode == Mode::PassThrough {
                return Ok(());
            }
            return Err(self.fail(io::ErrorKind::InvalidInput, "invalid UIDONLY command tag"));
        }

        match self.mode {
            Mode::PassThrough if command.eq_ignore_ascii_case(b"ENABLE UIDONLY") => {
                self.start_command(tag, CommandKind::Enable)?;
                self.mode = Mode::Enabling;
            }
            Mode::PassThrough => {}
            Mode::Enabling => {
                return Err(self.fail(
                    io::ErrorKind::InvalidInput,
                    "command sent before ENABLE UIDONLY completed",
                ));
            }
            Mode::Active => {
                let kind = if starts_ci(command, b"EXAMINE ") && command.len() > 8 {
                    CommandKind::Examine
                } else if command.eq_ignore_ascii_case(b"LOGOUT") {
                    CommandKind::Logout
                } else if let Some(kind) = uid_fetch_command_kind(command) {
                    kind
                } else {
                    return Err(self.fail(
                        io::ErrorKind::InvalidInput,
                        "command is not allowed after UIDONLY activation",
                    ));
                };
                self.start_command(tag, kind)?;
            }
            Mode::Poisoned => return Err(invalid("UIDONLY stream is poisoned")),
        }
        Ok(())
    }

    fn start_response(&mut self) -> io::Result<()> {
        self.in_response = true;
        self.first_line = true;
        self.response_uidfetch = false;
        self.response_bytes = 0;
        if let Some(command) = self.pending.as_mut() {
            command.response_count = command
                .response_count
                .checked_add(1)
                .ok_or_else(|| invalid("UIDONLY response count overflow"))?;
            if command.response_count > self.limits.max_command_responses {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "UIDONLY command response count exceeded",
                ));
            }
        }
        Ok(())
    }

    fn add_wire_bytes(&mut self, count: usize, literal: bool) -> io::Result<()> {
        self.response_bytes = self
            .response_bytes
            .checked_add(count)
            .ok_or_else(|| invalid("UIDONLY response byte count overflow"))?;
        if self.response_bytes > self.limits.max_response_bytes {
            return Err(self.fail(io::ErrorKind::InvalidData, "UIDONLY response too large"));
        }
        if let Some(command) = self.pending.as_mut() {
            command.response_bytes = command
                .response_bytes
                .checked_add(count)
                .ok_or_else(|| invalid("UIDONLY command byte count overflow"))?;
            if command.response_bytes > self.limits.max_command_response_bytes {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "UIDONLY command response bytes exceeded",
                ));
            }
        }
        if literal {
            let mut health = self.health.lock().expect("UIDONLY health poisoned");
            health.literal_bytes = health
                .literal_bytes
                .checked_add(count as u64)
                .ok_or_else(|| invalid("UIDONLY literal meter overflow"))?;
        }
        Ok(())
    }

    fn reserve_literal(&mut self, length: usize) -> io::Result<()> {
        if length > self.limits.max_literal_bytes {
            return Err(self.fail(io::ErrorKind::InvalidData, "UIDONLY literal too large"));
        }
        if self.response_bytes.saturating_add(length) > self.limits.max_response_bytes {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "UIDONLY literal exceeds response budget",
            ));
        }
        if let Some(command) = self.pending.as_mut() {
            if command.response_bytes.saturating_add(length)
                > self.limits.max_command_response_bytes
                || command.literal_bytes.saturating_add(length) > command.literal_limit
            {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "UIDONLY literal exceeds command budget",
                ));
            }
            command.literal_bytes += length;
        } else if self.mode == Mode::Active {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "UIDONLY literal arrived outside a command",
            ));
        }
        self.literal_remaining = length;
        Ok(())
    }

    fn classify_first_line(&mut self, line: &mut Vec<u8>) -> io::Result<()> {
        if self.mode == Mode::Enabling
            && numeric_atom(line).is_some_and(|atom| atom.eq_ignore_ascii_case(b"UIDFETCH"))
        {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "UIDFETCH arrived before UIDONLY activation",
            ));
        }
        if self.mode != Mode::Active {
            return Ok(());
        }
        if untagged_atom(line).is_some_and(|atom| atom.eq_ignore_ascii_case(b"VANISHED")) {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "VANISHED aborts UIDONLY acquisition",
            ));
        }
        if contains_ci(line, b"[UNSEEN ") {
            if self
                .pending
                .as_ref()
                .is_some_and(|command| command.kind == CommandKind::Examine)
                && starts_ci(line, b"* OK [UNSEEN ")
            {
                // Cyrus 3.12 advertises UIDONLY but still emits this optional
                // sequence-number response code on EXAMINE. Do not expose the
                // unusable sequence number to the parser or acquisition logic.
                *line = b"* OK UIDONLY ignored UNSEEN response code\r\n".to_vec();
            } else {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "sequence-bearing UNSEEN is forbidden in UIDONLY mode",
                ));
            }
        }
        if contains_ci(line, b"[UIDNOTSTICKY") {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "UIDNOTSTICKY is forbidden in UIDONLY mode",
            ));
        }
        if self.pending.as_ref().is_some_and(|command| {
            matches!(
                command.kind,
                CommandKind::Inventory | CommandKind::ExactFetch
            )
        }) && line.starts_with(b"* ")
            && !numeric_atom(line).is_some_and(|atom| atom.eq_ignore_ascii_case(b"UIDFETCH"))
            && !count_atom(line).is_some_and(|atom| {
                atom.eq_ignore_ascii_case(b"EXISTS") || atom.eq_ignore_ascii_case(b"RECENT")
            })
            && !untagged_atom(line).is_some_and(|atom| atom.eq_ignore_ascii_case(b"OK"))
        {
            let reason = if starts_ci(line, b"* NO [MESSAGELIMIT ") {
                "server reported MESSAGELIMIT during UIDONLY fetch"
            } else {
                "unexpected untagged response during UIDONLY fetch"
            };
            return Err(self.fail(io::ErrorKind::InvalidData, reason));
        }
        match numeric_atom_with_range(line) {
            Some((atom, start, end)) if atom.eq_ignore_ascii_case(b"UIDFETCH") => {
                if is_ignorable_uidfetch_notification(line)? {
                    *line = b"* OK UIDONLY ignored flag notification\r\n".to_vec();
                    return Ok(());
                }
                let kind = self.pending.as_ref().map(|command| command.kind);
                validate_uidfetch_shape(line, kind)
                    .map_err(|error| self.fail(io::ErrorKind::InvalidData, error.to_string()))?;
                let mut rewritten = Vec::with_capacity(line.len() - 3);
                rewritten.extend_from_slice(&line[..start]);
                rewritten.extend_from_slice(b"FETCH");
                rewritten.extend_from_slice(&line[end..]);
                *line = rewritten;
                self.response_uidfetch = true;
            }
            Some((atom, _, _)) if atom.eq_ignore_ascii_case(b"FETCH") => {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "raw FETCH is forbidden after UIDONLY activation",
                ));
            }
            Some((atom, _, _)) if atom.eq_ignore_ascii_case(b"EXPUNGE") => {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "sequence EXPUNGE is forbidden in UIDONLY mode",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_response(&mut self, line: &[u8]) -> io::Result<()> {
        let completion = tagged_completion(line);
        if self.mode == Mode::Enabling {
            if line.eq_ignore_ascii_case(b"* ENABLED UIDONLY\r\n") {
                if let Some(command) = self.pending.as_mut() {
                    command.saw_enabled = true;
                }
            } else if untagged_atom(line).is_some_and(|atom| atom.eq_ignore_ascii_case(b"ENABLED"))
            {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "server returned a non-exact ENABLED UIDONLY response",
                ));
            }
        }
        if self.mode != Mode::PassThrough {
            if let Some((tag, status)) = completion {
                let Some(command) = self.pending.as_ref() else {
                    return Err(self.fail(
                        io::ErrorKind::InvalidData,
                        "tagged completion arrived without a command",
                    ));
                };
                if tag != command.tag {
                    return Err(self.fail(
                        io::ErrorKind::InvalidData,
                        "tagged completion did not match the active command",
                    ));
                }
                if !status.eq_ignore_ascii_case(b"OK") {
                    return Err(self.fail(
                        io::ErrorKind::InvalidData,
                        "UIDONLY command did not complete OK",
                    ));
                }
                if command.kind == CommandKind::Enable && !command.saw_enabled {
                    return Err(self.fail(
                        io::ErrorKind::InvalidData,
                        "ENABLE completed without exact ENABLED UIDONLY",
                    ));
                }
                let enabled = command.kind == CommandKind::Enable;
                self.pending = None;
                self.health
                    .lock()
                    .expect("UIDONLY health poisoned")
                    .command_in_flight = false;
                if enabled {
                    self.mode = Mode::Active;
                    self.health.lock().expect("UIDONLY health poisoned").active = true;
                }
            }
        }
        self.in_response = false;
        self.first_line = true;
        self.response_uidfetch = false;
        self.response_bytes = 0;
        Ok(())
    }

    fn process_line(&mut self) -> io::Result<()> {
        let mut line = std::mem::take(&mut self.control_line);
        if self.first_line {
            self.classify_first_line(&mut line)?;
            self.first_line = false;
        } else if self.response_uidfetch && line != b")\r\n" {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "only closing ')' may follow an exact UIDFETCH literal",
            ));
        }
        let bare = &line[..line.len() - 2];
        let literal = literal_length(bare)?;
        if self.mode == Mode::Enabling && literal.is_some() {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "literal response is forbidden during UIDONLY activation",
            ));
        }
        if self.mode == Mode::Active && literal.is_some() && !self.response_uidfetch {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "literal outside UIDFETCH is forbidden in UIDONLY mode",
            ));
        }
        if let Some(length) = literal {
            self.reserve_literal(length)?;
            self.output.extend(line);
        } else {
            self.finish_response(&line)?;
            self.output.extend(line);
        }
        Ok(())
    }

    fn process_input(&mut self) -> io::Result<bool> {
        if !self.output.is_empty() {
            return Ok(true);
        }
        if !self.in_response && !self.input.is_empty() {
            self.start_response()?;
        }
        if self.literal_remaining > 0 {
            let count = self.literal_remaining.min(self.input.len()).min(CHUNK);
            if count == 0 {
                return Ok(false);
            }
            self.add_wire_bytes(count, true)?;
            self.input.make_contiguous();
            let (bytes, _) = self.input.as_slices();
            self.output.extend(&bytes[..count]);
            self.input.drain(..count);
            self.literal_remaining -= count;
            return Ok(true);
        }
        while let Some(byte) = self.input.pop_front() {
            self.control_line.push(byte);
            self.add_wire_bytes(1, false)?;
            if self.control_line.len() > self.limits.max_control_line_bytes {
                return Err(self.fail(io::ErrorKind::InvalidData, "UIDONLY control line too long"));
            }
            if byte == b'\n' {
                if !self.control_line.ends_with(b"\r\n") {
                    return Err(self.fail(io::ErrorKind::InvalidData, "bare LF in IMAP response"));
                }
                self.process_line()?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn copy_output(&mut self, destination: &mut ReadBuf<'_>) {
        self.output.make_contiguous();
        let (bytes, _) = self.output.as_slices();
        let count = destination.remaining().min(bytes.len());
        destination.put_slice(&bytes[..count]);
        self.output.drain(..count);
    }

    fn drain_outgoing(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>
    where
        T: AsyncWrite + Unpin,
    {
        while !self.outgoing.is_empty() {
            let written = {
                let (head, _) = self.outgoing.as_slices();
                match Pin::new(&mut self.inner).poll_write(cx, head) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(0)) => {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "failed to write UIDONLY command",
                        )))
                    }
                    Poll::Ready(Ok(written)) => written,
                }
            };
            self.outgoing.drain(..written);
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for UidOnlyStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        destination: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if destination.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if let Err(error) = this.check_deadline(cx) {
            return Poll::Ready(Err(error));
        }
        loop {
            if !this.output.is_empty() {
                this.copy_output(destination);
                return Poll::Ready(Ok(()));
            }
            match this.process_input() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => return Poll::Ready(Err(error)),
            }
            if this.eof {
                if this.in_response || !this.control_line.is_empty() || this.literal_remaining > 0 {
                    let error = this.fail(io::ErrorKind::UnexpectedEof, "truncated IMAP response");
                    return Poll::Ready(Err(error));
                }
                if this.pending.is_some() {
                    let error = this.fail(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed before UIDONLY command completion",
                    );
                    return Poll::Ready(Err(error));
                }
                return Poll::Ready(Ok(()));
            }
            let mut bytes = [0_u8; CHUNK];
            let mut buffer = ReadBuf::new(&mut bytes);
            match Pin::new(&mut this.inner).poll_read(cx, &mut buffer) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) if buffer.filled().is_empty() => this.eof = true,
                Poll::Ready(Ok(())) => this.input.extend(buffer.filled()),
            }
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for UidOnlyStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.mode == Mode::Poisoned {
            return Poll::Ready(Err(invalid("UIDONLY stream is poisoned")));
        }
        for byte in bytes {
            this.command_line.push(*byte);
            if this.command_line.len() > this.limits.max_control_line_bytes {
                let error = this.fail(io::ErrorKind::InvalidInput, "outbound IMAP line too long");
                return Poll::Ready(Err(error));
            }
            if *byte == b'\n' {
                let line = std::mem::take(&mut this.command_line);
                if let Err(error) = this.validate_command(&line) {
                    return Poll::Ready(Err(error));
                }
                this.outgoing.extend(line);
            }
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check_deadline(cx) {
            return Poll::Ready(Err(error));
        }
        if !this.command_line.is_empty() {
            return Poll::Ready(Err(this.fail(
                io::ErrorKind::InvalidInput,
                "flush attempted with a partial IMAP command",
            )));
        }
        match this.drain_outgoing(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.get_mut().inner).poll_shutdown(cx),
            other => other,
        }
    }
}

impl<T: SessionStream> SessionStream for UidOnlyStream<T> {}

/// Installs the bridge around Bichon's final (already TLS-upgraded) transport.
pub(crate) fn wrap(
    stream: Box<dyn SessionStream>,
    limits: UidOnlyLimits,
) -> io::Result<(Box<dyn SessionStream>, UidOnlyHandle)> {
    let (stream, handle) = UidOnlyStream::new(stream, limits)?;
    Ok((Box::new(stream), handle))
}

/// Injection-safe arguments for ascending RFC 9394 inventory paging.
pub(crate) fn inventory_args(
    cursor: u32,
    high: u32,
    page_size: u32,
) -> io::Result<(String, String)> {
    if cursor == 0 || high == 0 || cursor > high || page_size == 0 {
        return Err(invalid("invalid UIDONLY inventory bounds"));
    }
    Ok((
        format!("{cursor}:{high}"),
        format!("(UID RFC822.SIZE) (PARTIAL 1:{page_size})"),
    ))
}

/// Injection-safe arguments for one complete, literal-backed raw message.
pub(crate) fn exact_args(uid: u32) -> io::Result<(String, &'static str)> {
    if uid == 0 {
        return Err(invalid("UID 0 is invalid"));
    }
    Ok((uid.to_string(), "(UID RFC822.SIZE BODY.PEEK[])"))
}

fn uid_fetch_command_kind(command: &[u8]) -> Option<CommandKind> {
    let Some(rest) = strip_prefix_ci(command, b"UID FETCH ") else {
        return None;
    };
    let Some(space) = rest.iter().position(|byte| *byte == b' ') else {
        return None;
    };
    let (set, query) = (&rest[..space], &rest[space + 1..]);
    if let Some(colon) = set.iter().position(|byte| *byte == b':') {
        let Some(low) = parse_nonzero_u32(&set[..colon]) else {
            return None;
        };
        let Some(high) = parse_nonzero_u32(&set[colon + 1..]) else {
            return None;
        };
        if low > high {
            return None;
        }
        query
            .strip_prefix(b"(UID RFC822.SIZE) (PARTIAL 1:")
            .and_then(|tail| tail.strip_suffix(b")"))
            .and_then(parse_nonzero_u32)
            .map(|_| CommandKind::Inventory)
    } else {
        (parse_nonzero_u32(set).is_some() && query == b"(UID RFC822.SIZE BODY.PEEK[])")
            .then_some(CommandKind::ExactFetch)
    }
}

fn validate_uidfetch_shape(line: &[u8], command: Option<CommandKind>) -> io::Result<()> {
    let bare = line
        .strip_suffix(b"\r\n")
        .ok_or_else(|| invalid("invalid UIDFETCH response"))?;
    let rest = bare
        .strip_prefix(b"* ")
        .ok_or_else(|| invalid("invalid UIDFETCH response"))?;
    let number_end = rest
        .iter()
        .position(|byte| *byte == b' ')
        .ok_or_else(|| invalid("invalid UIDFETCH response"))?;
    let leading_uid = parse_nonzero_u32(&rest[..number_end])
        .ok_or_else(|| invalid("invalid leading UIDFETCH UID"))?;
    let attributes = strip_prefix_ci(&rest[number_end + 1..], b"UIDFETCH ")
        .and_then(|value| value.strip_prefix(b"("))
        .ok_or_else(|| invalid("invalid UIDFETCH attribute list"))?;

    let (attributes, exact) = match command {
        Some(CommandKind::Inventory) => (
            attributes
                .strip_suffix(b")")
                .ok_or_else(|| invalid("inventory UIDFETCH must not contain a literal"))?,
            false,
        ),
        Some(CommandKind::ExactFetch) => {
            literal_length(bare)?
                .ok_or_else(|| invalid("exact UIDFETCH BODY[] must be literal-backed"))?;
            let marker = attributes
                .iter()
                .rposition(|byte| *byte == b'{')
                .ok_or_else(|| invalid("exact UIDFETCH is missing its literal marker"))?;
            (&attributes[..marker], true)
        }
        _ => return Err(invalid("UIDFETCH arrived outside a UID fetch command")),
    };

    let mut uid = None;
    let mut size = None;
    let mut body = false;
    let mut tokens = attributes
        .split(|b| b.is_ascii_whitespace())
        .filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case(b"UID") && uid.is_none() {
            uid = tokens.next().and_then(parse_nonzero_u32);
            if uid.is_none() {
                return Err(invalid("UIDFETCH has an invalid or duplicate UID"));
            }
        } else if token.eq_ignore_ascii_case(b"RFC822.SIZE") && size.is_none() {
            size = tokens.next().and_then(parse_u32);
            if size.is_none() {
                return Err(invalid("UIDFETCH has an invalid or duplicate RFC822.SIZE"));
            }
        } else if exact && token.eq_ignore_ascii_case(b"BODY[]") && !body {
            body = true;
            if tokens.next().is_some() {
                return Err(invalid("BODY[] must be the final exact UIDFETCH attribute"));
            }
        } else {
            return Err(invalid("unexpected or duplicate UIDFETCH attribute"));
        }
    }
    let uid = uid.ok_or_else(|| invalid("UIDFETCH omitted UID"))?;
    size.ok_or_else(|| invalid("UIDFETCH omitted RFC822.SIZE"))?;
    if uid != leading_uid {
        return Err(invalid("leading UIDFETCH UID disagrees with UID attribute"));
    }
    if body != exact {
        return Err(invalid("UIDFETCH body did not match command"));
    }
    Ok(())
}

fn is_ignorable_uidfetch_notification(line: &[u8]) -> io::Result<bool> {
    let bare = line
        .strip_suffix(b"\r\n")
        .ok_or_else(|| invalid("invalid UIDFETCH notification"))?;
    if literal_length(bare)?.is_some() {
        return Ok(false);
    }
    let Some((atom, _, end)) = numeric_atom_with_range(line) else {
        return Ok(false);
    };
    if !atom.eq_ignore_ascii_case(b"UIDFETCH") {
        return Ok(false);
    }
    let attributes = bare[end..]
        .strip_prefix(b" (")
        .and_then(|value| value.strip_suffix(b")"))
        .ok_or_else(|| invalid("invalid UIDFETCH notification attributes"))?;
    Ok(contains_ci(attributes, b"FLAGS")
        && !contains_ci(attributes, b"RFC822.SIZE")
        && !contains_ci(attributes, b"BODY"))
}

fn literal_length(line: &[u8]) -> io::Result<Option<usize>> {
    if !line.ends_with(b"}") {
        return Ok(None);
    }
    let Some(open) = line.iter().rposition(|byte| *byte == b'{') else {
        return Err(invalid("invalid IMAP literal marker"));
    };
    if open > 0 && line[open - 1] == b'~' {
        return Err(invalid("literal8 is unsupported"));
    }
    let digits = &line[open + 1..line.len() - 1];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(invalid("invalid IMAP literal marker"));
    }
    let length = std::str::from_utf8(digits)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| invalid("IMAP literal length overflow"))?;
    Ok(Some(length))
}

fn numeric_atom_with_range(line: &[u8]) -> Option<(&[u8], usize, usize)> {
    numbered_atom_with_range(line, false)
}

fn numbered_atom_with_range(line: &[u8], allow_zero: bool) -> Option<(&[u8], usize, usize)> {
    let bare = line.strip_suffix(b"\r\n")?;
    let rest = bare.strip_prefix(b"* ")?;
    let number_end = rest.iter().position(|byte| *byte == b' ')?;
    if allow_zero {
        parse_u32(&rest[..number_end])?;
    } else {
        parse_nonzero_u32(&rest[..number_end])?;
    }
    let atom_start = 2 + number_end + 1;
    let tail = &bare[atom_start..];
    let atom_end = tail
        .iter()
        .position(|byte| *byte == b' ' || *byte == b'\r' || *byte == b'\n')
        .unwrap_or(tail.len());
    Some((&tail[..atom_end], atom_start, atom_start + atom_end))
}

fn numeric_atom(line: &[u8]) -> Option<&[u8]> {
    numeric_atom_with_range(line).map(|value| value.0)
}

fn count_atom(line: &[u8]) -> Option<&[u8]> {
    numbered_atom_with_range(line, true).map(|value| value.0)
}

fn untagged_atom(line: &[u8]) -> Option<&[u8]> {
    let bare = line.strip_suffix(b"\r\n")?;
    let rest = bare.strip_prefix(b"* ")?;
    let end = rest
        .iter()
        .position(|byte| *byte == b' ')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn tagged_completion(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let bare = line.strip_suffix(b"\r\n")?;
    if bare.starts_with(b"* ") || bare.starts_with(b"+ ") {
        return None;
    }
    let mut fields = bare.split(|byte| *byte == b' ');
    let tag = fields.next()?;
    let status = fields.next()?;
    if [b"OK".as_slice(), b"NO".as_slice(), b"BAD".as_slice()]
        .iter()
        .any(|candidate| status.eq_ignore_ascii_case(candidate))
    {
        Some((tag, status))
    } else {
        None
    }
}

fn parse_nonzero_u32(bytes: &[u8]) -> Option<u32> {
    parse_u32(bytes).filter(|number| *number != 0)
}

fn parse_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn starts_ci(value: &[u8], prefix: &[u8]) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn strip_prefix_ci<'a>(value: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    starts_ci(value, prefix).then(|| &value[prefix.len()..])
}

fn contains_ci(value: &[u8], needle: &[u8]) -> bool {
    value
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn invalid(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.into())
}

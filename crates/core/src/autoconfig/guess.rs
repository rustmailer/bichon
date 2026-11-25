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

use std::time::Duration;

use crate::account::entity::Encryption;
use crate::autoconfig::client::{IncomingServer, MailConfig};
use crate::imap::client::Client;
use crate::utils::ssrf;
use tracing::{debug, info};

/// A guess is a connectivity probe, not a real sync session; fail fast.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A single host:port:encryption combination to probe.
struct Guess {
    hostname: String,
    port: u16,
    encryption: Encryption,
    socket_type: &'static str,
}

/// Generate candidates in the same order Thunderbird uses:
/// 1. imap.{domain}          — most common
/// 2. mail.{domain}          — fallback
/// 3. {domain}               — bare domain (rare)
fn make_guesses(domain: &str) -> Vec<Guess> {
    let hosts = [
        format!("imap.{domain}"),
        format!("mail.{domain}"),
        domain.to_string(),
    ];

    let mut guesses = Vec::with_capacity(hosts.len() * 2);
    for host in &hosts {
        guesses.push(Guess {
            hostname: host.clone(),
            port: 993,
            encryption: Encryption::Ssl,
            socket_type: "SSL",
        });
        guesses.push(Guess {
            hostname: host.clone(),
            port: 143,
            encryption: Encryption::StartTls,
            socket_type: "STARTTLS",
        });
    }
    guesses
}

/// Try to open a connection, read the IMAP banner, and close.
/// Returns `true` if the server responds with an IMAP greeting.
async fn probe(hostname: &str, port: u16, encryption: &Encryption) -> bool {
    match tokio::time::timeout(
        PROBE_TIMEOUT,
        Client::connection(hostname, encryption, port, None, true),
    )
    .await
    {
        Ok(Ok(_)) => {
            debug!("GuessConfig probe succeeded: {hostname}:{port} ({encryption:?})");
            true
        }
        Ok(Err(e)) => {
            debug!("GuessConfig probe failed for {hostname}:{port}: {e:?}");
            false
        }
        Err(_) => {
            debug!("GuessConfig probe timed out for {hostname}:{port}");
            false
        }
    }
}

/// Thunderbird-style guessing: try common hostnames and ports, probing
/// each with a real TCP connection.
///
/// Returns the first working `MailConfig`, or `None` if nothing works.
pub async fn guess_config(domain: &str) -> Option<MailConfig> {
    if !ssrf::hostname_is_acceptable(domain) {
        return None;
    }
    let guesses = make_guesses(domain);
    info!("GuessConfig: trying {} candidates for {domain}", guesses.len());

    for g in &guesses {
        // A probe is a real TCP connection; never aim it at a local address.
        // The name is checked again at connection time by the resolver, but
        // requiring a public resolution up front keeps internal hosts out of
        // the candidate list in the first place.
        if !ssrf::host_resolves_public(&g.hostname, g.port).await {
            debug!("GuessConfig: skipping non-public host {}", g.hostname);
            continue;
        }
        if probe(&g.hostname, g.port, &g.encryption).await {
            info!(
                "GuessConfig: found working IMAP at {}:{} ({})",
                g.hostname, g.port, g.socket_type
            );
            return Some(MailConfig {
                incoming: vec![IncomingServer {
                    protocol: "imap".to_string(),
                    hostname: g.hostname.clone(),
                    port: g.port,
                    socket_type: g.socket_type.to_string(),
                    username: "%EMAILADDRESS%".to_string(),
                    authentication: String::new(),
                }],
                outgoing: vec![],
            });
        }
    }

    None
}

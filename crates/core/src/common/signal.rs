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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use crate::{context::Initialize, error::BichonResult, utils::shutdown::shutdown_signal};
use tokio::sync::broadcast;

pub static SIGNAL_MANAGER: LazyLock<SignalManager> = LazyLock::new(SignalManager::new);

pub struct SignalManager {
    sender: broadcast::Sender<()>,
    /// Set once a shutdown has been requested, so subscribers that appear
    /// later (e.g. the HTTP server during a slow startup) still see it.
    shutdown_requested: AtomicBool,
}

impl SignalManager {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1);
        SignalManager {
            sender,
            shutdown_requested: AtomicBool::new(false),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        let receiver = self.sender.subscribe();
        // The broadcast channel drops messages with no receiver. A shutdown
        // requested before this subscriber existed (disk watchdog firing
        // during startup, for example) must not be lost.
        if self.shutdown_requested.load(Ordering::Acquire) {
            let _ = self.sender.send(());
        }
        receiver
    }

    /// Request a graceful shutdown from inside the process. Idempotent.
    pub fn trigger_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Release);
        let _ = self.sender.send(());
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
    }
}

impl Initialize for SignalManager {
    async fn initialize() -> BichonResult<()> {
        tokio::spawn({
            async move {
                shutdown_signal().await;
                println!("\nSending shutdown signal...");
                SIGNAL_MANAGER.trigger_shutdown();
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_before_subscribe_is_not_lost() {
        let manager = SignalManager::new();
        manager.trigger_shutdown();
        let mut receiver = manager.subscribe();
        assert!(receiver.try_recv().is_ok());
        assert!(manager.is_shutdown_requested());
    }
}

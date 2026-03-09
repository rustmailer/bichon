//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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

use std::sync::LazyLock;

use crate::modules::{context::Initialize, error::BichonResult, utils::shutdown::shutdown_signal};
use tokio::sync::broadcast;

pub static SIGNAL_MANAGER: LazyLock<SignalManager> = LazyLock::new(SignalManager::new);

pub struct SignalManager {
    sender: broadcast::Sender<()>,
}

impl SignalManager {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1);
        SignalManager { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }
}

impl Initialize for SignalManager {
    async fn initialize() -> BichonResult<()> {
        tokio::spawn({
            async move {
                shutdown_signal().await;
                println!("\nSending shutdown signal...");
                let _ = SIGNAL_MANAGER.sender.send(());
            }
        });
        Ok(())
    }
}

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

use dashmap::DashMap;
use governor::{
    clock::{QuantaClock, QuantaInstant},
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
    NotUntil, Quota, RateLimiter,
};
use std::{
    num::NonZero,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::users::acl::RateLimit;

pub static RATE_LIMITER_MANAGER: LazyLock<UserRateLimiter> = LazyLock::new(UserRateLimiter::new);

pub struct UserRateLimiter {
    limiters: Arc<
        DashMap<
            u64,
            (
                Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware>>,
                RateLimit,
            ),
        >,
    >,
}

impl UserRateLimiter {
    pub fn new() -> Self {
        UserRateLimiter {
            limiters: Arc::new(DashMap::new()),
        }
    }

    pub async fn check(
        &self,
        user_id: u64,
        limit: RateLimit,
    ) -> Result<(), NotUntil<QuantaInstant>> {
        let limiter = self.get_or_update_limiter(user_id, limit).await;
        limiter.check()
    }

    async fn get_or_update_limiter(
        &self,
        user_id: u64,
        limit: RateLimit,
    ) -> Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware>> {
        self.limiters
            .entry(user_id)
            .and_modify(|(existing_limiter, current_limit)| {
                if current_limit.interval != limit.interval || current_limit.quota != limit.quota {
                    let quota = Quota::with_period(Duration::from_secs(limit.interval))
                        .unwrap()
                        .allow_burst(NonZero::new(limit.quota).unwrap());
                    *existing_limiter = Arc::new(RateLimiter::direct_with_clock(
                        quota,
                        QuantaClock::default(),
                    ));
                    *current_limit = RateLimit {
                        interval: limit.interval,
                        quota: limit.quota,
                    };
                }
            })
            .or_insert({
                let quota = Quota::with_period(Duration::from_secs(limit.interval))
                    .unwrap()
                    .allow_burst(NonZero::new(limit.quota).unwrap());
                (
                    Arc::new(RateLimiter::direct_with_clock(
                        quota,
                        QuantaClock::default(),
                    )),
                    limit,
                )
            })
            .value()
            .0
            .clone()
    }
}

pub static LOGIN_RATE_LIMITER_MANAGER: LazyLock<LoginRateLimiter> = LazyLock::new(LoginRateLimiter::new);

pub struct LoginRateLimiter {
    limiters: Arc<
        DashMap<
            String,
            Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware>>,
        >,
    >,
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        LoginRateLimiter {
            limiters: Arc::new(DashMap::new()),
        }
    }

    pub async fn check(&self, ip: &str) -> Result<(), NotUntil<QuantaInstant>> {
        let limiter = self.limiters.entry(ip.to_string()).or_insert_with(|| {
            let quota = Quota::with_period(Duration::from_secs(600))
                .unwrap()
                .allow_burst(NonZero::new(10).unwrap());
            Arc::new(RateLimiter::direct_with_clock(quota, QuantaClock::default()))
        });
        limiter.value().check()
    }
}

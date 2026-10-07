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
//

//! Maintenance mode: when startup fails before the web server can start
//! (legacy data layout requiring a `bichon-admin` migration, license
//! preflight failure, ...), the process stays alive and answers every HTTP
//! request with a simple 503 page instead of exiting.
//!
//! Exiting is the worst option under Docker: the container stops, and
//! `docker exec` no longer works, which is the only practical way for a
//! normal user to run the interactive `bichon-admin` migration. In
//! maintenance mode the process keeps running (`docker exec -it
//! <container> bichon-admin` works), the page tells the user what to do,
//! and `/api/status` returns 503 so the Dockerfile healthcheck marks the
//! container unhealthy while it remains reachable.

use std::sync::Arc;
use std::time::Duration;

use bichon_core::{
    bichon_version,
    common::signal::{SignalManager, SIGNAL_MANAGER},
    context::Initialize,
    settings::cli::SETTINGS,
};
use poem::{
    listener::TcpListener,
    Endpoint, Request, Response, Route, Server,
};
use tracing::{error, info, warn};

/// How often the "still in maintenance mode" reminder is written to the
/// log, so `docker logs --tail` keeps showing the reason for the outage.
const REMINDER_INTERVAL_SECS: u64 = 600;

/// Serve the maintenance page and block until SIGTERM/SIGINT.
///
/// `edition` labels the build ("Community Edition", "Pro / Enterprise
/// Edition", ...). The exact license edition is deliberately NOT resolved
/// here: maintenance mode typically triggers before the license status is
/// loaded, or precisely because it cannot be used, so only the binary-level
/// branding is a reliable label.
///
/// Callers reach this before `initialize()` has run, so nothing here may
/// touch databases, TLS material, or anything beyond the bind address:
/// keeping the process alive is the whole point (see module docs). If the
/// HTTP server cannot start (e.g. the port is taken), the process stays
/// alive anyway and waits for a shutdown signal.
pub async fn serve_maintenance(edition: &str, heading: &str, steps: &[String]) {
    // The signal watcher is normally installed by `initialize()` further
    // down the startup path; start it early so `docker stop` shuts this
    // down gracefully instead of waiting out the SIGKILL timeout.
    if let Err(e) = SignalManager::initialize().await {
        error!("Failed to install signal handlers: {e:#?}");
    }

    let bind_ip = SETTINGS.bichon_bind_ip.clone().unwrap_or("0.0.0.0".into());
    let port = SETTINGS.bichon_http_port as u16;

    warn!(
        "Entering maintenance mode: serving a 503 page on {bind_ip}:{port} \
         until the problem below is fixed ({}).",
        heading
    );
    if SETTINGS.bichon_enable_rest_https {
        warn!(
            "HTTPS is enabled, but the maintenance page is served over plain \
             HTTP: TLS is not initialized before the startup checks pass."
        );
    }

    // Periodic reminder in the logs, for users who look at `docker logs`
    // instead of (or before) opening the web UI.
    let reminder_heading = heading.to_string();
    let reminder = tokio::spawn(async move {
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(REMINDER_INTERVAL_SECS),
            Duration::from_secs(REMINDER_INTERVAL_SECS),
        );
        loop {
            ticker.tick().await;
            warn!("Maintenance mode still active: {reminder_heading}");
        }
    });

    let page = MaintenancePage {
        html: Arc::new(render_page(edition, heading, steps)),
    };
    let app = Route::new().at("/", page.clone()).at("/*", page);

    let mut shutdown_rx = SIGNAL_MANAGER.subscribe();
    let server = Server::new(TcpListener::bind((bind_ip, port)))
        .name("Bichon Maintenance")
        .idle_timeout(Duration::from_secs(60))
        .run_with_graceful_shutdown(
            app,
            async move {
                let _ = shutdown_rx.recv().await;
            },
            Some(Duration::from_secs(5)),
        );

    match server.await {
        Ok(()) => info!("Maintenance server stopped."),
        Err(e) => {
            error!("Maintenance-mode HTTP server failed: {e:#?}");
            error!(
                "Staying alive anyway so `docker exec -it <container> \
                 bichon-admin` keeps working; stop the process to exit."
            );
            let mut rx = SIGNAL_MANAGER.subscribe();
            let _ = rx.recv().await;
        }
    }
    reminder.abort();
}

/// Answers every request with the same 503 page.
#[derive(Clone)]
struct MaintenancePage {
    html: Arc<String>,
}

impl Endpoint for MaintenancePage {
    type Output = Response;

    async fn call(&self, _req: Request) -> poem::Result<Self::Output> {
        Ok(Response::builder()
            .status(http::StatusCode::SERVICE_UNAVAILABLE)
            .header("Retry-After", "300")
            .content_type("text/html; charset=utf-8")
            .body(self.html.as_str().to_owned())
            .into())
    }
}

fn render_page(edition: &str, heading: &str, steps: &[String]) -> String {
    let steps_html = steps
        .iter()
        .map(|s| format!("<li>{}</li>", escape_html(s)))
        .collect::<Vec<_>>()
        .join("\n            ");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Bichon — {heading}</title>
<style>
  body {{ margin: 0; font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
         background: #f5f5f5; color: #24292f; }}
  main {{ max-width: 640px; margin: 8vh auto; padding: 32px 40px;
         background: #fff; border: 1px solid #d0d7de; border-radius: 8px; }}
  h1 {{ margin: 0 0 4px; font-size: 28px; letter-spacing: 1px; }}
  .edition {{ font-size: 13px; font-weight: 500; letter-spacing: 0;
            vertical-align: middle; margin-left: 10px; padding: 2px 10px;
            border: 1px solid #d0d7de; border-radius: 999px; color: #57606a; }}
  h2 {{ margin: 0 0 16px; font-size: 18px; color: #b35900; }}
  ol {{ padding-left: 20px; line-height: 1.7; }}
  li {{ margin-bottom: 6px; }}
  code {{ background: #f0f2f4; padding: 1px 6px; border-radius: 4px;
         font-size: 0.92em; }}
  .footer {{ margin-top: 20px; font-size: 12px; color: #656d76; }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #161a1d; color: #e6edf3; }}
    main {{ background: #1f2428; border-color: #3d444d; }}
    code {{ background: #2d333b; }}
    .footer {{ color: #9198a1; }}
    .edition {{ border-color: #3d444d; color: #9198a1; }}
  }}
</style>
</head>
<body>
  <main>
    <h1>Bichon<span class="edition">{edition}</span></h1>
    <h2>{heading}</h2>
    <p>The Bichon service is not running. Follow the steps below, then restart
       the service.</p>
    <ol>
            {steps}
    </ol>
    <p class="footer">Version {version} &middot;
       Documentation: https://github.com/rustmailer/bichon/wiki</p>
  </main>
</body>
</html>
"#,
        edition = escape_html(edition),
        heading = escape_html(heading),
        steps = steps_html,
        version = bichon_version!(),
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_escapes_markup_and_renders_steps() {
        let html = render_page(
            "Community <Edition>",
            "Migration <required>",
            &["Run <bichon-admin> & restart".to_string()],
        );
        assert!(html.contains("Community &lt;Edition&gt;"));
        assert!(html.contains("Migration &lt;required&gt;"));
        assert!(html.contains("<li>Run &lt;bichon-admin&gt; &amp; restart</li>"));
    }

    #[test]
    fn escape_covers_all_entities() {
        assert_eq!(
            escape_html(r#"<a href="x">&"</a> "#),
            r#"&lt;a href=&quot;x&quot;&gt;&amp;&quot;&lt;/a&gt; "#
        );
    }
}

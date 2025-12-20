<div align="center">

<h1 align="center">
  <img width="200" height="175" alt="image" src="https://github.com/user-attachments/assets/06dc3b67-7d55-4a93-a3de-8b90951c575b" />
  <br>
  Bichon
  <br>
</h1>

<h3 align="center">
  A lightweight, high-performance Rust email archiver with WebUI
</h3>

<p style="display: flex; gap: 10px; justify-content: center; flex-wrap: wrap;">
  <a href="https://github.com/rustmailer/bichon/releases">
    <img src="https://img.shields.io/github/v/release/rustmailer/bichon" alt="Release">
  </a>
  <a href="https://hub.docker.com/r/rustmailer/bichon">
    <img src="https://img.shields.io/docker/v/rustmailer/bichon?label=docker" alt="Docker">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-AGPLv3-blue.svg" alt="License">
  </a>
  <a href="https://deepwiki.com/rustmailer/bichon"><img src="https://deepwiki.com/badge.svg" alt="Ask DeepWiki"></a>
  <a href="https://discord.gg/Bq4M2cDmF4">
    <img src="https://img.shields.io/badge/Discord-Join%20Server-7289DA?logo=discord&logoColor=white" alt="Discord">
  </a>
  <a href="https://x.com/rustmailer">
    <img src="https://img.shields.io/twitter/follow/rustmailer?style=social" alt="Follow on X">
  </a>
</p>
</div>

Bichon is an open-source email archiving system that **synchronizes emails from IMAP servers**, **indexes them for full-text search**, and provides a **REST API** for programmatic access.
**Unlike email clients**, Bichon is designed for **archiving and searching** rather than sending/receiving emails. It runs as a **standalone server application** that continuously synchronizes configured email accounts and maintains a **searchable local archive**.
Built in Rust, it requires no external dependencies and provides fast, efficient email archiving, management, and search through a built-in WebUI. Its name is inspired by the puppy my daughter adopted last month.

## Key Differences from Email Clients

### Core Comparison

| Feature | Email Clients | Bichon |
|---------|---------------|--------|
| **Primary Purpose** | Send/receive emails, real-time communication | Archive, search, manage historical emails |
| **Sending Capability** | ✅ Supports sending emails | ❌ No email sending support |
| **Runtime Mode** | Desktop/mobile applications | Server-side application |
| **Data Storage** | Local cache + server | Local archive store |
| **Search Capability** | Basic search | Full-text indexing, advanced search |
| **API Interface** | Typically not provided | Complete REST API |
| **Multi-account Management** | Limited | Supports unified search across accounts |


## 🚀 Features

### ⚡ Lightweight & Standalone
- Pure Rust, single-machine application.  
- No external database required.  
- Includes **WebUI** for intuitive management.

### 📬 Multi-Account Management
- Synchronize and download emails from multiple accounts.  
- Flexible selection: by **date range**, **number of emails**, or **specific mailboxes**.

### 🔑 IMAP & OAuth2 Authentication
- Supports **IMAP password** or **OAuth2** login.  
- Built-in WebUI for **OAuth2 authorization**, including **automatic token refresh** (e.g., Gmail, Outlook).  
- Supports **network proxy** for IMAP and OAuth2.  
- Automatic IMAP server discovery and configuration.

### 🔍 Unified Multi-Account Search
- Powerful search across all accounts:  
  **account**, **mailbox**, **sender**, **attachment name**, **has attachments**, **size**, **date**, **subject**, **body**.

### 🏷️ Tags & Facets
- Organize archived emails using **tags** backed by Tantivy **facets**.  
- Efficiently filter and locate emails based on these facet-based tags.

### 💾 Compressed & Deduplicated Storage
- Store emails efficiently with **transparent compression** and **deduplication**—emails can be read directly without any extra steps.

### 📂 Email Management & Viewing
- Bulk cleanup of local archives.  
- Download emails as **EML** or **attachments separately**.  
- View and browse emails directly.
- View the full **conversation thread** of any email.

### 📊 Dashboard & Analytics
- Visualize email statistics: **counts**, **time distribution**, **top senders**, **largest emails**, **account rankings**.

### 🌐 Internationalization (i18n)
* WebUI fully supports **17 languages** for all interface elements.
* Backend responses (e.g., system messages, API data) are **not yet internationalized**.
* Frontend is ready to support more languages in the future with minimal effort.

### 🛠️ OpenAPI Support
- Provides **OpenAPI documentation**.  
- **Access token authentication** for programmatic access.

## 🐾 Why Create Bichon?

A few months ago, I released **rustmailer**, an email API middleware:  
https://github.com/rustmailer/rustmailer

Since then, I’ve received many emails asking whether it could also archive emails, perform unified search, and support full-text indexing—not just querying recipients.  
But rustmailer was designed as a middleware focused on providing API services.  
Adding archiving and full-text search would complicate its core purpose and go far beyond its original scope.

Meanwhile, I realized that email archiving itself only requires a small portion of rustmailer’s functionality, plus a search engine.  
With that combination, building a dedicated, efficient archiver becomes much simpler.

Using the experience gained from rustmailer, I designed and built **Bichon** in less than two weeks, followed by another two weeks of testing and optimization.  
It has now reached a stable, usable state—and I decided to release it publicly.

**Bichon is completely free**.  
You can download and use it however you like.  
It’s not perfect, but I hope it brings you value.
## 📸 Snapshot
<img width="1914" height="904" alt="image" src="https://github.com/user-attachments/assets/3a456999-e4eb-441e-9052-3a727dea66a0" />
<img width="1900" height="907" alt="image" src="https://github.com/user-attachments/assets/95db0a05-4b55-4e18-b418-9d40361d6fea" />
<img width="1912" height="904" alt="image" src="https://github.com/user-attachments/assets/96b0ebc2-4778-452b-891f-dc9acf8e381f" />
<img width="1909" height="904" alt="image" src="https://github.com/user-attachments/assets/ab4bf6ae-faa6-4b49-ae39-705eb9d4487f" />
<img width="1910" height="910" alt="image" src="https://github.com/user-attachments/assets/bcf9cca2-d690-4e7b-b2c9-c52a31c7b999" />
<img width="1915" height="903" alt="image" src="https://github.com/user-attachments/assets/242817d7-3e12-4cbb-afb0-c5ef7366178d" />
<img width="1920" height="910" alt="image" src="https://github.com/user-attachments/assets/14561b74-ed53-4017-9c5b-a64920ec3526" />
<img width="1913" height="909" alt="image" src="https://github.com/user-attachments/assets/6fd54cb0-c86f-4ceb-a955-c81107614fc4" />
<img width="1916" height="814" alt="image" src="https://github.com/user-attachments/assets/6a079d98-ff6c-46f4-9ec6-e76d320bff5d" />

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=rustmailer/bichon&type=date&legend=top-left)](https://www.star-history.com/#rustmailer/bichon&type=date&legend=top-left)


## 🚀 Quick Start

### Docker Deployment (Recommended)

```bash
# Pull the image
docker pull rustmailer/bichon:latest

# Create data directory
mkdir -p ./bichon-data

# Run container
docker run -d \
  --name bichon \
  -p 15630:15630 \
  -v $(pwd)/bichon-data:/data \
  -e BICHON_LOG_LEVEL=info \
  -e BICHON_ROOT_DIR=/data \
  rustmailer/bichon:latest
```

## CORS Configuration (Important for Browser Access)

Starting from **v0.1.4**, Bichon changes how `BICHON_CORS_ORIGINS` works:

### **🔄 New Behavior in v0.1.4**

* If **`BICHON_CORS_ORIGINS` is not set**, Bichon now **allows all origins**.
  This makes local testing and simple deployments much easier.
* If you **do set** `BICHON_CORS_ORIGINS`, then **you must explicitly list each allowed origin**.
* `*` is **not supported** and will **not work** — you must provide exact URLs.

#### How CORS Matching Works

When a browser accesses Bichon, it will send an `Origin` header.

* **Incoming Origin** = the exact address the browser is using
* **Configured origins** = the list you passed to `BICHON_CORS_ORIGINS`

If Configured origins does not contain the Incoming Origin exactly as a full string match, the browser request will be rejected.

Example debug log:

```
2025-12-06T23:56:30.422+08:00 DEBUG bichon::modules::rest: CORS: Incoming Origin = "http://localhost:15630"
2025-12-06T23:56:30.422+08:00 DEBUG bichon::modules::rest: CORS: Configured origins = ["http://192.168.3.2:15630"]
```

In this example:

* Browser is using `http://localhost:15630`
* But the configured origin is `http://192.168.3.2:15630`

→ **CORS will fail**, and you can immediately see why.

#### When Should You Configure CORS?

It is strongly recommended to configure CORS in production environments to ensure that only trusted browser origins can access Bichon.
If you want to access Bichon from a browser:

* Add the exact **IP** with port
* Or the exact **hostname** with port
* Or the **domain** (port optional if it's 80)

Examples:

```
http://192.168.1.16:15630
http://myserver.local:15630
http://mydomain.com
```

If you access Bichon in **multiple different ways**, list all of them:

```
-e BICHON_CORS_ORIGINS="http://192.168.1.16:15630,http://myserver.local:15630,http://mydomain.com"
```

> **Do not add a trailing slash**
> (`http://192.168.1.16:15630/` will not match)
>
> **Do not use `*`**, it is not supported.

#### How to Enable Debug Logs (Highly Recommended for CORS Issues)

Set environment variable:

```
BICHON_LOG_LEVEL=debug
```

Or via command-line:

```
--bichon-log-level debug
```

Default is `info`, so CORS logs will not appear unless debug logging is enabled.

---

#### ⚠️ Note on Running Bichon in a Container

> ⚠️ **Note:** If you are running Bichon in a container (via **Docker Compose** or **docker run**), be careful with **quotes in environment variable values**.

For example, **do not** write:

```bash
-e BICHON_CORS_ORIGINS="http://localhost:15630,http://myserver.local:15630"
```

* The outer quotes (`"`) will be passed literally into the container and may cause CORS misconfiguration.

**Correct way:**

```bash
-e BICHON_CORS_ORIGINS=http://localhost:15630,http://myserver.local:15630
```

Or using YAML literal style for Docker Compose:

```yaml
environment:
  BICHON_CORS_ORIGINS: |
    http://localhost:15630,http://myserver.local:15630
```

This ensures that the configured origins are interpreted correctly inside the container.

> ⚠️ **Note:** This fucking problem I actually didn’t know about myself; thanks to [gall-1](https://github.com/gall-1) for pointing it out.


### Binary Deployment

Download the appropriate binary for your platform from the [Releases](https://github.com/rustmailer/bichon/releases) page:

- Linux (GNU): `bichon-x.x.x-x86_64-unknown-linux-gnu.tar.gz`
- Linux (MUSL): `bichon-x.x.x-x86_64-unknown-linux-musl.tar.gz`
- macOS: `bichon-x.x.x-x86_64-apple-darwin.tar.gz`
- Windows: `bichon-x.x.x-x86_64-pc-windows-msvc.zip`

Extract and run:

```bash
# Linux/macOS
./bichon --bichon-root-dir /tmp/bichon-data

# Windows
.\bichon.exe --bichon-root-dir e:\bichon-data
```

* --bichon-root-dir argument is required and must be an absolute path.

* If you are accessing Bichon from a proxy domain **mydomain** argument --bichon-cors-origins="https://mydomain" is required.
  
## Setting the Bichon Encryption Password

Bichon uses an encryption password to secure sensitive data. **You must set it before first use**, when no data exists.

Once set, it **cannot be changed**. Changing it later will make all encrypted data unreadable. To start over, you would need to **reinitialize Bichon and clear all emails and metadata**.

### How to Set the Password

You can set the password **via command-line or environment variable**:

### Command-Line

```bash
bichon --bichon-encrypt-password "your-strong-password"
```

### Environment Variable

```bash
export BICHON_ENCRYPT_PASSWORD="your-strong-password"
bichon
```

**Tip:** Use a strong, secure password and keep it safe, as it cannot be changed later.

## 🔑 Root User Login Information

**Bichon currently supports a single Root user login for system access and management.**

### First Login and Enabling Access

To enable the login feature, you must specify a command-line argument or set an environment variable when starting Bichon.

#### 1\. Command-Line Argument

Add the `--bichon-enable-access-token` flag to your startup command:

```bash
# Linux/macOS Binary Deployment Example
./bichon --bichon-root-dir /tmp/bichon-data --bichon-enable-access-token
```

#### 2\. Environment Variable (Recommended for Docker)

Set the environment variable `BICHON_ENABLE_ACCESS_TOKEN` to `true`:

```bash
# Docker Deployment Example
docker run -d \
  --name bichon \
  -p 15630:15630 \
  -v $(pwd)/bichon-data:/data \
  -e BICHON_LOG_LEVEL=info \
  -e BICHON_ROOT_DIR=/data \
  -e BICHON_ENABLE_ACCESS_TOKEN=true \
  rustmailer/bichon:latest
```

### Default Credentials

  * **Initial Login Account:** `root`
  * **Initial Password:** `root`

### Changing the Password

**It is strongly recommended that you change the default password immediately after your first login.**

You can change the password via the WebUI:

1.  Log in to the WebUI.
2.  Navigate to the **Settings** page.
3.  Use the **Reset Root Password** option to modify your password.


## 📖 Documentation

> Under construction. Documentation will be available soon.
[Bichon Wiki](https://github.com/rustmailer/bichon/wiki).

## FAQ

please see the FAQ in the project Wiki:

👉 [https://github.com/rustmailer/bichon/wiki/FAQ](https://github.com/rustmailer/bichon/wiki/FAQ-(Frequently-Asked-Questions))


## 💡 User Case Showcase

We have collected a real-world case study from a user processing email data, which demonstrates Bichon's performance and storage efficiency in a live environment.
This case involves ingesting and indexing data from **126 email accounts**. The total original data volume was **229 GB**, comprising **460,000 emails**.

### 📊 Performance Data Overview

<img width="945" height="582" alt="image" src="https://github.com/user-attachments/assets/934ed6dd-c1da-4483-84fa-6d5b1bf6ca72" />

A special thank you to **[@rallisf1](https://github.com/rallisf1)** for sharing this usage scenario and the detailed data.

#### 🤝 Open Invitation

This data is provided solely as a **reference** for real-world usage. We encourage more users to share their Bichon usage screenshots and metrics (e.g., ingestion volume, compression ratio, search speed, etc.) to help the community conduct a more comprehensive assessment of Bichon's suitability and performance.

---

## Roadmap

* [ ] Multi-user support with account/password login

  * System-level roles (admin / user)
  * Per-mail-account permissions

* [ ] `bichon-cli` command-line tool

  * Import emails from `eml`, `mbox`, `msg`, `pst`

* [ ] Manual sync controls

  * Sync on demand
  * Sync a single folder
  * Verify completeness by comparing with the mail server

* [ ] Post-sync server cleanup

  * Clean up server-side emails after successful sync
  * Free up mailbox space (e.g. Gmail)

* [ ] Email export

  * Export by folder
  * Export by entire account

* [ ] Account-to-account email sync

  * Sync emails to a specified target account
  * Support mailbox migration

---

## 🛠️ Tech Stack

- **Backend**: Rust + Poem
- **Frontend**: React + TypeScript + Vite + ShadCN UI
- **Core Engine (Storage & Search)**: Tantivy
  - Acts as both the primary storage for email content and the full-text search index. This unified approach ensures high performance and eliminates data redundancy.
- **Metadata Storage**: Native_DB
  - Used exclusively for lightweight configuration and account metadata.
- **Email Protocols**: IMAP (Supports standard Password & OAuth2)

## 🤝 Contributing

Contributions of all kinds are welcome!
Whether you’d like to submit code, report a bug, or share practical suggestions that can help improve the project, your input is highly appreciated.
Feel free to open an Issue or a Pull Request anytime. You can also reach out on Discord if you’d like to discuss ideas or improvements.
<a href="https://discord.gg/Bq4M2cDmF4">
    <img src="https://img.shields.io/badge/Discord-Join%20Server-7289DA?logo=discord&logoColor=white" alt="Discord">
</a>

## 🧑‍💻 Developer Guide

To build or contribute to Bichon, the following environment is recommended:

### Prerequisites
- **Rust**: Use the latest stable toolchain for best compatibility and performance.
- **Node.js**: Version **20+** is required.
- **pnpm**: Recommended package manager for the WebUI.

### Steps

#### 1. Clone the repository
```bash
git clone https://github.com/rustmailer/bichon.git
cd bichon
```

#### 2. Build the WebUI

```bash
cd web
pnpm install
pnpm run build
```

Run the WebUI in development mode if needed:

```bash
pnpm run dev
```

#### 3. Build or Run the Backend

After the WebUI is built, return to the project root:

```bash
cd ..
cargo build
```

Or run directly:

```bash
export BICHON_ENCRYPT_PASSWORD=dummy-password-for-testing
cargo run -- --bichon-root-dir e:\bichon-data
```

`--bichon-root-dir` specifies the directory where **all Bichon data** will be stored.
`BICHON_ENCRYPT_PASSWORD` is the password used to encrypt the sensitive data (see `cargo run -- --help` for alternative ways to specify this).

### WebUI Access

* The WebUI runs on **[http://localhost:15630](http://localhost:15630)** by default.
* **HTTPS is not enabled** in development or default builds.  

<cite/>

## 📄 License

This project is licensed under [AGPLv3](LICENSE).

## 🔗 Links

- [Docker Hub](https://hub.docker.com/r/rustmailer/bichon)
- [Issue Tracker](https://github.com/rustmailer/bichon/issues)
- [Discord](https://discord.gg/Bq4M2cDmF4)


## 💖 Support & Promotion

If this project has been helpful to you and you’d like to support its development, you can consider making a small donation or helping spread the word.
Financial support is optional but deeply appreciated — it helps me dedicate more time and resources to building new features and improving the overall experience.

You can also support the project by sharing it with others, writing about your experience, or recommending it within relevant communities. Every bit of visibility helps more people benefit from the tool!

[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Support%20the%20Project-FFDD00?logo=buy-me-a-coffee)](https://buymeacoffee.com/rustmailer)

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
  <a href="https://discord.gg/evFnSpdpaE">
    <img src="https://img.shields.io/badge/Discord-Join%20Server-7289DA?logo=discord&logoColor=white" alt="Discord">
  </a>
  <a href="https://x.com/rustmailer">
    <img src="https://img.shields.io/twitter/follow/rustmailer?style=social" alt="Follow on X">
  </a>
</p>
</div>

Bichon is a minimal, high-performance, standalone Rust email archiver with a built-in WebUI.
Its name is inspired by the puppy my daughter adopted last month.
It runs as a single binary, requires no external dependencies, and provides fast, efficient email archiving, management, and search.

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

* If you are accessing Bichon on the same machine where it is installed (Machine A), open:
  ```
  http://localhost:15630
  ```

* If you are accessing Bichon from another machine (Machine B), make sure to set CORS with the IP of Machine B, for example:

```bash
# Run container
docker run -d \
  --name bichon \
  -p 15630:15630 \
  -v $(pwd)/bichon-data:/data \
  -e BICHON_LOG_LEVEL=info \
  -e BICHON_ROOT_DIR=/data \
  -e BICHON_CORS_ORIGINS="http://localhost:15630,http://B_MACHINE_IP:15630,*" \
  rustmailer/bichon:latest
```
Access instructions:
This allows Machine B to access the Bichon interface on Machine A via a browser.

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


## 🛠️ Tech Stack

- **Backend**: Rust + Poem
- **Frontend**: React + TypeScript + Vite + ShadCN UI
- **Core Engine (Storage & Search)**: Tantivy
  - Acts as both the primary storage for email content and the full-text search index. This unified approach ensures high performance and eliminates data redundancy.
- **Metadata Storage**: Native_DB
  - Used exclusively for lightweight configuration and account metadata.
- **Email Protocols**: IMAP (Supports standard Password & OAuth2)


## 🤝 Contributing

Issues and Pull Requests are welcome!

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
````

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
cargo run -- --bichon-root-dir e:\bichon-data
```
`--bichon-root-dir` specifies the directory where **all Bichon data** will be stored.

### WebUI Access

* The WebUI runs on **[http://localhost:15630](http://localhost:15630)** by default.
* **HTTPS is not enabled** in development or default builds.  

<cite/>

## 📄 License

This project is licensed under [AGPLv3](LICENSE).

## 🔗 Links

- [Docker Hub](https://hub.docker.com/r/rustmailer/bichon)
- [Issue Tracker](https://github.com/rustmailer/bichon/issues)
- [Discord](https://discord.gg/evFnSpdpaE)


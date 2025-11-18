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
  <a href="https://deepwiki.com/rustmailer/bichon">
    <img src="https://deepwiki.com/badge-maker?url=https%3A%2F%2Fdeepwiki.com%2Frustmailer%2Fbichon" alt="DeepWiki">
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
  -e RUSTMAILER_LOG_LEVEL=info \
  -e RUSTMAILER_ROOT_DIR=/data \
  rustmailer/bichon:latest
```

Access `http://localhost:15630` to start using Bichon.

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


## 📖 Documentation

> Under construction. Documentation will be available soon.


## 🛠️ Tech Stack

- **Backend**: Rust + Poem
- **Frontend**: React + TypeScript + Vite + ShadCN
- **Storage**: Native_DB
- **Search Engine**: Tantivy
- **Email Protocols**: IMAP (Password & OAuth2)


## 🤝 Contributing

Issues and Pull Requests are welcome!

<cite/>

## 📄 License

This project is licensed under [AGPLv3](LICENSE).

## 🔗 Links

- [Official Website](https://rustmailer.com)
- [Docker Hub](https://hub.docker.com/r/rustmailer/bichon)
- [Issue Tracker](https://github.com/rustmailer/bichon/issues)

---

<div align="center">
Made with ❤️ by rustmailer.com
</div>

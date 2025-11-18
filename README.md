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

- [Official Website](https://rustmailer.com)
- [Docker Hub](https://hub.docker.com/r/rustmailer/bichon)
- [Issue Tracker](https://github.com/rustmailer/bichon/issues)

---

<div align="center">
Made with ❤️ by rustmailer.com
</div>

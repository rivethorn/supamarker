# Hermes-MD

![Crates.io Version](https://img.shields.io/crates/v/supamarker)

A CLI tool for adding and removing Markdown files to and from a Supabase bucket. I use it for my blog site, you can use it for whatever.

## ⚠️ Warning

**Never publish your Service Role Key** — it WILL grant unlimited power over your Supabase project to anyone who has it.

## Installation

You can install it from [crates.io](https://crates.io/crates/supamarker):

```bash
cargo install hermes-md
```

Or you can clone the repository and build it from source:

```bash
git clone https://github.com/rivethorn/hermes-md.git
cd hermes-md
cargo build --release
```

## Usage

```bash
hermes-md publish <path>         # upload file + metadata
hermes-md list                   # show slugs and where they are (bucket/table/both)
hermes-md delete <slug>          # delete file + row after confirmation
hermes-md delete <slug> --soft   # delete only DB row (keeps bucket file)
hermes-md gen-config             # write sample config to platform-specific config directory
```

## Configuration

### Config File (Preferred)

Place `config.toml` in the current directory, or in the platform-specific config directory:

- **Unix/Linux**: `~/.config/hermes-md/config.toml` (or `$XDG_CONFIG_HOME/hermes-md/config.toml` if set)
- **Windows**: `%APPDATA%\hermes-md\config.toml` (or `%LOCALAPPDATA%\hermes-md\config.toml`)

Override the path with `--config /path/to/config.toml` (or `--config C:\path\to\config.toml` on Windows).

Example `config.toml`:

```toml
supabase_url = "https://xxxxx.supabase.co"
supabase_service_key = "service_role_key"
bucket = "blog"
table = "posts"
```

### Environment Variables

Environment variables (`SUPABASE_URL`, `SUPABASE_SERVICE_KEY`, `SUPABASE_BUCKET`, `SUPABASE_TABLE`) are honored as a fallback if no config file is found.

### To-do

- Add GUI

# Supamarker

![Crates.io Version](https://img.shields.io/crates/v/supamarker)


A CLI tool for adding and removing Markdown files to and from a Supabase bucket. I use it for my blog site, you can use it for whatever.

## ⚠️ Warning

**Never publish your Service Role Key** — it WILL grant unlimited power over your Supabase project to anyone who has it.

## Installation

You can install it from [crates.io](https://crates.io/crates/supamarker):

```bash
cargo install supamarker
```

Or you can clone the repository and build it from source:

```bash
git clone https://github.com/rivethorn/supamarker.git
cd supamarker
cargo build --release
```

## Usage

```bash
supamarker publish <path>         # upload file + metadata
supamarker list                   # show slugs and where they are (bucket/table/both)
supamarker delete <slug>          # delete file + row after confirmation
supamarker delete <slug> --soft   # delete only DB row (keeps bucket file)
supamarker gen-config             # write sample config to ~/.config/supamarker/config.toml
```

## Configuration

### Config File (Preferred)

Place `config.toml` in the current directory, or `~/.config/supamarker/config.toml`. Override the path with `--config /path/to/config.toml`.

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

- Make sure of Windows support
- Add GUI

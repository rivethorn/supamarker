# Hermes

> Typescript rewrite
> Rust was NOT the best choice for this

A CLI tool for adding and removing Markdown files to and from a Supabase bucket. I use it for my blog site, you can use it for whatever.

## ⚠️ Warning

**Never publish your Service Role Key** — it WILL grant unlimited power over your Supabase project to anyone who has it.

## Usage

```bash
bun run hermes.ts publish <path>         # upload file + metadata
bun run hermes.ts list                   # show slugs and where they are (bucket/table/both)
bun run hermes.ts delete <slug>          # delete file + row after confirmation
bun run hermes.ts delete <slug> --soft   # delete only DB row (keeps bucket file)
bun run hermes.ts gen-config             # write sample config to platform-specific config directory
```

## Configuration

### Config File (Preferred)

Place `config.toml` in the current directory.

Override the path with `--config /path/to/config.toml` (or `--config C:\path\to\config.toml` on Windows).

Example `config.toml`:

```toml
supabase_url = "https://xxxxx.supabase.co"
supabase_service_key = "service_role_key"
bucket = "blog"
```

### Environment Variables

Environment variables (`SUPABASE_URL`, `SUPABASE_SERVICE_KEY`, `SUPABASE_BUCKET`) are honored as a fallback if no config file is found.

use std::{
    collections::HashSet,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::multipart;
use serde::Deserialize;
use slug::slugify;
use zenity::spinner::MultiSpinner;

#[derive(Parser)]
#[command(name = "supamarker")]
#[command(about = "Publish markdown posts to Supabase (storage + posts table)")]
struct Cli {
    /// Optional path to a config file (TOML). If set, this is used first.
    #[arg(long, global = true)]
    config: Option<String>,
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish a local markdown file
    Publish { path: String },
    /// Delete a post by slug
    Delete {
        slug: String,
        /// Remove only the database row; keep the file in the bucket
        #[arg(long)]
        soft: bool,
    },
    /// List slugs and where they exist
    List,
    /// Generate a sample config at the default path
    GenConfig,
}

#[derive(Debug, Deserialize)]
struct FrontMatter {
    title: String,
    summary: Option<String>,
    tags: Option<Vec<String>>,
    slug: Option<String>,
}

async fn publish(
    supabase_url: &str,
    service_key: &str,
    bucket: &str,
    table: &str,
    path: &str,
) -> Result<()> {
    let spinner = MultiSpinner::default();
    let sid = spinner.get_last();
    spinner.set_text(&sid, "Preparing file...".to_string());

    // 1) Read file
    let md = fs::read_to_string(path).with_context(|| format!("reading {}", path))?;

    // 2) Extract frontmatter (simple YAML between --- markers)
    //    We'll try to find `---\n...yaml...\n---\n` at start
    let (fm_opt, _) = parse_frontmatter(&md)?;
    let fm = fm_opt
        .ok_or_else(|| anyhow!("Frontmatter not found or invalid. Provide YAML frontmatter."))?;

    // 3) Slug
    let slug = fm.slug.clone().unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| slugify(s))
            .unwrap_or_else(|| slugify(&fm.title))
    });

    // 4) Upload markdown file to Supabase Storage via REST API
    // Endpoint: POST {SUPABASE_URL}/storage/v1/object/{bucket}/{object}
    // (multipart/form-data field "file")
    let upload_url = format!(
        "{}/storage/v1/object/{}/{}.md",
        supabase_url.trim_end_matches('/'),
        bucket,
        slug
    );

    let client = reqwest::Client::new();

    let file_name = format!("{}.md", slug);
    let part = multipart::Part::text(md.clone())
        .file_name(file_name)
        .mime_str("text/markdown")?;
    // note: we use "file" field like the JS SDK/multipart examples do
    let form = multipart::Form::new().part("file", part);

    spinner.set_text(&sid, "Uploading markdown to storage...".to_string());

    let upload_resp = client
        .post(&upload_url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        // recommended Accept header
        .header(ACCEPT, "application/json")
        .multipart(form)
        .send()
        .await
        .with_context(|| "uploading markdown to Supabase Storage")?;

    if !upload_resp.status().is_success() {
        let status = upload_resp.status();
        let text = upload_resp.text().await.unwrap_or_default();
        return Err(anyhow!("Storage upload failed: {} - {}", status, text));
    }

    println!("✓ uploaded markdown to storage as {}/{}.md", bucket, slug);

    spinner.set_text(&sid, "Upserting metadata...".to_string());

    // 5) Upsert metadata into your table via PostgREST (Supabase REST)
    // Use the PostgREST endpoint: {SUPABASE_URL}/rest/v1/{SUPABASE_TABLE}
    // We'll POST and set "Prefer: resolution=merge-duplicates" so conflict = upsert (merge)
    let rest_url = format!("{}/rest/v1/{}", supabase_url.trim_end_matches('/'), table);

    // Build JSON payload (we send an array with a single row)
    let payload = serde_json::json!([{
        "slug": slug,
        "title": fm.title,
        "summary": fm.summary.unwrap_or_default(),
        "tags": fm.tags.unwrap_or_default()
    }]);

    let metadata_resp = client
        .post(&rest_url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        // required by Supabase PostgREST to identify project and allow the key
        .header("apikey", service_key)
        // ask PostgREST to merge duplicates (upsert)
        .header("Prefer", "resolution=merge-duplicates")
        .header(CONTENT_TYPE, "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("inserting/upserting metadata into {} table", table))?;

    if !metadata_resp.status().is_success() {
        let status = metadata_resp.status();
        let text = metadata_resp.text().await.unwrap_or_default();
        return Err(anyhow!("DB upsert failed: {} - {}", status, text));
    }

    spinner.set_text(&sid, "Done.".to_string());
    drop(spinner);

    println!(
        "✓ upserted metadata into {} table for slug `{}`",
        table, slug
    );
    println!("Published ✅: {}", fm.title);

    Ok(())
}

async fn delete_post(
    supabase_url: &str,
    service_key: &str,
    bucket: &str,
    slug: &str,
    table: &str,
    soft: bool,
) -> Result<()> {
    let normalized_slug = normalize_slug(slug);
    let client = reqwest::Client::new();

    let spinner = MultiSpinner::default();
    let sid = spinner.get_last();
    spinner.set_text(&sid, format!("Verifying `{}`...", normalized_slug));

    let storage_exists =
        check_storage_presence(&client, supabase_url, service_key, bucket, &normalized_slug)
            .await?;
    let table_exists =
        check_table_presence(&client, supabase_url, service_key, table, &normalized_slug).await?;

    if !storage_exists && !table_exists {
        drop(spinner);
        return Err(anyhow!(
            "Slug `{}` not found in storage or table; nothing to delete",
            normalized_slug
        ));
    }

    spinner.set_text(
        &sid,
        format!(
            "Found in: storage={} table={}",
            storage_exists, table_exists
        ),
    );
    drop(spinner); // stop animation before prompting

    if !prompt_confirm(&format!(
        "Delete `{}`{}?",
        normalized_slug,
        if soft {
            " (soft delete: keep bucket file)"
        } else {
            ""
        }
    ))? {
        println!("Aborted.");
        return Ok(());
    }

    // 1) Delete markdown from storage
    if !soft && storage_exists {
        let spinner = MultiSpinner::default();
        let sid = spinner.get_last();
        spinner.set_text(
            &sid,
            format!("Deleting markdown from storage: {}.md...", normalized_slug),
        );

        let storage_url = format!(
            "{}/storage/v1/object/{}/{}.md",
            supabase_url.trim_end_matches('/'),
            bucket,
            normalized_slug
        );

        let storage_resp = client
            .delete(&storage_url)
            .header(AUTHORIZATION, format!("Bearer {}", service_key))
            .header("apikey", service_key) // needed for service role
            .header("Accept", "application/json")
            .send()
            .await?;

        if !storage_resp.status().is_success() {
            let status = storage_resp.status();
            let text = storage_resp.text().await.unwrap_or_default();
            drop(spinner);
            return Err(anyhow!(
                "Failed to delete storage file: {} - {}",
                status,
                text
            ));
        }

        spinner.set_text(&sid, "Deleted from storage.".to_string());
        drop(spinner);
        println!(
            "✓ Deleted markdown from storage: {}/{}.md",
            bucket, normalized_slug
        );
    }

    // 2) Delete metadata from DB
    if table_exists {
        let spinner = MultiSpinner::default();
        let sid = spinner.get_last();
        spinner.set_text(&sid, format!("Deleting metadata from `{}`...", table));

        let rest_url = format!(
            "{}/rest/v1/{}?slug=eq.{}",
            supabase_url.trim_end_matches('/'),
            table,
            normalized_slug
        );

        let db_resp = client
            .delete(&rest_url)
            .header(AUTHORIZATION, format!("Bearer {}", service_key))
            .header("apikey", service_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !db_resp.status().is_success() {
            let status = db_resp.status();
            let text = db_resp.text().await.unwrap_or_default();
            drop(spinner);
            return Err(anyhow!(
                "Failed to delete metadata from DB: {} - {}",
                status,
                text
            ));
        }

        spinner.set_text(&sid, "Deleted metadata.".to_string());
        drop(spinner);

        println!(
            "✓ Deleted metadata from {} table for slug `{}`",
            table, normalized_slug
        );
    }

    println!("Post `{}` deleted successfully ✅", normalized_slug);

    Ok(())
}

/// Very small frontmatter parser: returns (Option<FrontMatter>, content_without_fm)
fn parse_frontmatter(s: &str) -> Result<(Option<FrontMatter>, String)> {
    let s = s.trim_start();
    if !s.starts_with("---") {
        return Ok((None, s.to_string()));
    }

    // find second '---' marker
    let mut parts = s.splitn(3, "---");
    // first split gives empty before first '---'
    let _ = parts.next();
    let yaml = parts
        .next()
        .ok_or_else(|| anyhow!("no closing frontmatter marker"))?;
    let rest = parts.next().unwrap_or("");

    let yaml = yaml.trim();
    let rest = rest.trim_start_matches('\n').to_string();

    let fm: FrontMatter = serde_yaml::from_str(yaml).context("parsing YAML frontmatter")?;
    Ok((Some(fm), rest))
}

#[derive(Deserialize)]
struct FileConfig {
    supabase_url: Option<String>,
    supabase_service_key: Option<String>,
    bucket: Option<String>,
    table: Option<String>,
}

struct ResolvedConfig {
    supabase_url: String,
    service_key: String,
    bucket: String,
    table: String,
}

fn candidate_config_paths(cli_path: Option<&str>) -> Vec<PathBuf> {
    if let Some(p) = cli_path {
        return vec![PathBuf::from(p)];
    }

    let mut paths = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("config.toml"));
    }

    #[cfg(unix)]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            paths.push(PathBuf::from(xdg).join("supamarker/config.toml"));
        } else if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join(".config/supamarker/config.toml"));
        }
    }

    #[cfg(windows)]
    {
        // On Windows, prefer APPDATA (roaming) or LOCALAPPDATA (local)
        if let Ok(appdata) = std::env::var("APPDATA") {
            paths.push(PathBuf::from(appdata).join("supamarker/config.toml"));
        } else if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            paths.push(PathBuf::from(localappdata).join("supamarker/config.toml"));
        } else if let Ok(userprofile) = std::env::var("USERPROFILE") {
            // Fallback to USERPROFILE\.config\supamarker\config.toml
            paths.push(PathBuf::from(userprofile).join(".config/supamarker/config.toml"));
        }
    }

    paths
}

fn read_config_file(path: &Path) -> Result<FileConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let cfg: FileConfig = toml::from_str(&raw).context("parsing TOML config")?;
    Ok(cfg)
}

fn load_config(cli_config: Option<&str>) -> Result<ResolvedConfig> {
    let mut file_cfg: Option<FileConfig> = None;
    let candidates = candidate_config_paths(cli_config);

    for p in &candidates {
        if p.exists() {
            file_cfg = Some(read_config_file(p)?);
            break;
        }
    }

    let supabase_url = file_cfg
        .as_ref()
        .and_then(|c| c.supabase_url.clone())
        .or_else(|| std::env::var("SUPABASE_URL").ok())
        .ok_or_else(|| {
            anyhow!("Missing supabase_url. Set it in a config file or SUPABASE_URL env var.")
        })?;

    let service_key = file_cfg
        .as_ref()
        .and_then(|c| c.supabase_service_key.clone())
        .or_else(|| std::env::var("SUPABASE_SERVICE_KEY").ok())
        .ok_or_else(|| anyhow!("Missing supabase_service_key. Set it in a config file or SUPABASE_SERVICE_KEY env var."))?;

    let bucket = file_cfg
        .as_ref()
        .and_then(|c| c.bucket.clone())
        .or_else(|| std::env::var("SUPABASE_BUCKET").ok())
        .unwrap_or_else(|| "blog".to_string());

    let table = file_cfg
        .as_ref()
        .and_then(|c| c.table.clone())
        .or_else(|| std::env::var("SUPABASE_TABLE").ok())
        .unwrap_or_else(|| "posts".to_string());

    Ok(ResolvedConfig {
        supabase_url,
        service_key,
        bucket,
        table,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    let args = Cli::parse();

    match args.cmd {
        Commands::Publish { path } => {
            let config = load_config(args.config.as_deref())?;
            publish(
                &config.supabase_url,
                &config.service_key,
                &config.bucket,
                &config.table,
                &path,
            )
            .await?;
        }
        Commands::Delete { slug, soft } => {
            let config = load_config(args.config.as_deref())?;
            delete_post(
                &config.supabase_url,
                &config.service_key,
                &config.bucket,
                &slug,
                &config.table,
                soft,
            )
            .await?;
        }
        Commands::List => {
            let config = load_config(args.config.as_deref())?;
            list_items(
                &config.supabase_url,
                &config.service_key,
                &config.bucket,
                &config.table,
            )
            .await?;
        }
        Commands::GenConfig => {
            let path = gen_config()?;
            println!(
                "Sample config written to {}. Update the values before running publish/list/delete.",
                path.display()
            );
        }
    }

    Ok(())
}

fn default_config_path() -> Result<PathBuf> {
    #[cfg(unix)]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(xdg).join("supamarker/config.toml"));
        }

        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(".config/supamarker/config.toml"));
        }

        return Err(anyhow!(
            "HOME not set; cannot determine default config path"
        ));
    }

    #[cfg(windows)]
    {
        // On Windows, prefer APPDATA (roaming) or LOCALAPPDATA (local)
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("supamarker/config.toml"));
        }

        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            return Ok(PathBuf::from(localappdata).join("supamarker/config.toml"));
        }

        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            // Fallback to USERPROFILE\.config\supamarker\config.toml
            return Ok(PathBuf::from(userprofile).join(".config/supamarker/config.toml"));
        }

        return Err(anyhow!(
            "APPDATA, LOCALAPPDATA, or USERPROFILE not set; cannot determine default config path"
        ));
    }

    #[cfg(not(any(unix, windows)))]
    {
        Err(anyhow!(
            "Unsupported platform; cannot determine default config path"
        ))
    }
}

fn gen_config() -> Result<PathBuf> {
    let path = default_config_path()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }

    if path.exists() {
        return Err(anyhow!(
            "Config already exists at {}. Delete or move it to regenerate.",
            path.display()
        ));
    }

    let sample = r#"supabase_url = "https://xxxxx.supabase.co"
supabase_service_key = "service_role_key"
bucket = "blog"
table = "posts"
"#;

    fs::write(&path, sample).with_context(|| format!("writing config to {}", path.display()))?;

    Ok(path)
}

fn normalize_slug(input: &str) -> String {
    input.trim_end_matches(".md").to_string()
}

fn prompt_confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let resp = input.trim().to_lowercase();
    Ok(resp == "y" || resp == "yes")
}

async fn check_storage_presence(
    client: &reqwest::Client,
    supabase_url: &str,
    service_key: &str,
    bucket: &str,
    slug: &str,
) -> Result<bool> {
    let url = format!(
        "{}/storage/v1/object/{}/{}.md",
        supabase_url.trim_end_matches('/'),
        bucket,
        slug
    );

    let resp = client
        .head(url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        .header("apikey", service_key)
        .send()
        .await?;

    Ok(resp.status().is_success())
}

#[derive(Deserialize)]
struct TableRow {
    slug: String,
}

async fn check_table_presence(
    client: &reqwest::Client,
    supabase_url: &str,
    service_key: &str,
    table: &str,
    slug: &str,
) -> Result<bool> {
    let url = format!(
        "{}/rest/v1/{}?slug=eq.{}&select=slug",
        supabase_url.trim_end_matches('/'),
        table,
        slug
    );

    let resp = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        .header("apikey", service_key)
        .header("Accept", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Failed to check table presence: {} - {}",
            status,
            text
        ));
    }

    let rows: Vec<TableRow> = resp.json().await?;
    Ok(!rows.is_empty())
}

#[derive(Deserialize)]
struct StorageObject {
    name: String,
}

async fn fetch_storage_slugs(
    client: &reqwest::Client,
    supabase_url: &str,
    service_key: &str,
    bucket: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "{}/storage/v1/object/list/{}",
        supabase_url.trim_end_matches('/'),
        bucket
    );

    let resp = client
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        .header("apikey", service_key)
        .header("Accept", "application/json")
        .header(CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({ "prefix": "" }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Failed to list storage objects: {} - {}",
            status,
            text
        ));
    }

    let files: Vec<StorageObject> = resp.json().await?;
    Ok(files.into_iter().map(|f| normalize_slug(&f.name)).collect())
}

async fn fetch_table_slugs(
    client: &reqwest::Client,
    supabase_url: &str,
    service_key: &str,
    table: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "{}/rest/v1/{}?select=slug",
        supabase_url.trim_end_matches('/'),
        table
    );

    let resp = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {}", service_key))
        .header("apikey", service_key)
        .header("Accept", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to list table rows: {} - {}", status, text));
    }

    let rows: Vec<TableRow> = resp.json().await?;
    Ok(rows.into_iter().map(|r| normalize_slug(&r.slug)).collect())
}

async fn list_items(
    supabase_url: &str,
    service_key: &str,
    bucket: &str,
    table: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    let spinner = MultiSpinner::default();
    let sid = spinner.get_last();
    spinner.set_text(&sid, "Fetching storage objects...".to_string());

    let storage_slugs = fetch_storage_slugs(&client, supabase_url, service_key, bucket).await?;
    spinner.set_text(&sid, "Fetching table rows...".to_string());

    let table_slugs = fetch_table_slugs(&client, supabase_url, service_key, table).await?;
    spinner.set_text(&sid, "Computing differences...".to_string());
    drop(spinner);

    let storage_set: HashSet<String> = storage_slugs.into_iter().collect();
    let table_set: HashSet<String> = table_slugs.into_iter().collect();

    let mut all: Vec<String> = storage_set.union(&table_set).cloned().collect();
    all.sort();

    if all.is_empty() {
        println!("No slugs found in storage bucket or table.");
        return Ok(());
    }

    println!("{:<32}{}", "slug", "location");
    for slug in all {
        let in_storage = storage_set.contains(&slug);
        let in_table = table_set.contains(&slug);
        let location = match (in_storage, in_table) {
            (true, true) => "both",
            (true, false) => "bucket",
            (false, true) => "table",
            (false, false) => "missing",
        };
        println!("{:<32}{}", slug, location);
    }

    Ok(())
}

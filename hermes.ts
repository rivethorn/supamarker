#!/usr/bin/env node

import { readFile, writeFile, mkdir } from "fs/promises";
import fs from "fs";
import path from "path";
import process from "process";
import { Command } from "commander";
import dotenv from "dotenv";
import yaml from "js-yaml";
import slugify from "slugify";
import { createClient, SupabaseClient } from "@supabase/supabase-js";

dotenv.config();

/* -------------------------------------------------------------------------- */
/* Types                                                                      */
/* -------------------------------------------------------------------------- */

interface FrontMatter {
  title: string;
  tag: string;
  ttr: string;
  slug?: string;
}

interface FileConfig {
  supabase_url?: string;
  supabase_service_key?: string;
  bucket?: string;
  table?: string;
}

interface ResolvedConfig {
  supabaseUrl: string;
  serviceKey: string;
  bucket: string;
  table: string;
}

/* -------------------------------------------------------------------------- */
/* CLI                                                                         */
/* -------------------------------------------------------------------------- */

const program = new Command();

program
  .name("hermes")
  .description("Publish markdown posts to Supabase (storage + posts table)")
  .option("--config <path>", "Path to config file");

program
  .command("publish")
  .argument("<path>", "Markdown file")
  .action(async (filePath, opts) => {
    const cfg = await loadConfig(program.opts().config);
    await publish(cfg, filePath);
  });

program
  .command("delete")
  .argument("<slug>", "Post slug")
  .option("--soft", "Keep storage file")
  .action(async (slug, options) => {
    const cfg = await loadConfig(program.opts().config);
    await deletePost(cfg, slug, options.soft);
  });

program.command("list").action(async () => {
  const cfg = await loadConfig(program.opts().config || "./config.toml");
  await listItems(cfg);
});

program.command("gen-config").action(async () => {
  const p = await genConfig();
  console.log(`Sample config written to ${p}`);
});

program.parse();

/* -------------------------------------------------------------------------- */
/* Supabase                                                                    */
/* -------------------------------------------------------------------------- */

function supabaseClient(cfg: ResolvedConfig): SupabaseClient {
  return createClient(cfg.supabaseUrl, cfg.serviceKey, {
    auth: { persistSession: false },
  });
}

/* -------------------------------------------------------------------------- */
/* Publish                                                                     */
/* -------------------------------------------------------------------------- */

async function publish(cfg: ResolvedConfig, filePath: string) {
  const md = await readFile(filePath, "utf8");

  const { frontmatter } = parseFrontmatter(md);
  if (!frontmatter) {
    throw new Error("Missing or invalid frontmatter");
  }

  const slug =
    frontmatter.slug ??
    slugify(path.basename(filePath, path.extname(filePath)), { lower: true });

  const supabase = supabaseClient(cfg);

  /* Upload markdown */
  const { error: uploadErr } = await supabase.storage
    .from(cfg.bucket)
    .upload(`${slug}.md`, md, {
      upsert: true,
      contentType: "text/markdown",
    });

  if (uploadErr) throw uploadErr;

  /* Upsert metadata */
  const { error: dbErr } = await supabase.from(cfg.table).upsert(
    {
      slug,
      title: frontmatter.title,
      tag: frontmatter.tag,
      time_to_read: frontmatter.ttr,
    },
    { onConflict: "slug" }
  );

  if (dbErr) throw dbErr;

  console.log(`✓ Published: ${frontmatter.title}`);
}

/* -------------------------------------------------------------------------- */
/* Delete                                                                      */
/* -------------------------------------------------------------------------- */

async function deletePost(
  cfg: ResolvedConfig,
  inputSlug: string,
  soft = false
) {
  const slug = normalizeSlug(inputSlug);
  const supabase = supabaseClient(cfg);

  const { data: file } = await supabase.storage
    .from(cfg.bucket)
    .list("", { search: `${slug}.md` });

  const inStorage = !!file?.length;

  const { data: rows } = await supabase
    .from(cfg.table)
    .select("slug")
    .eq("slug", slug);

  const inTable = !!rows?.length;

  if (!inStorage && !inTable) {
    throw new Error(`Slug '${slug}' not found`);
  }

  if (!soft && inStorage) {
    const { error } = await supabase.storage
      .from(cfg.bucket)
      .remove([`${slug}.md`]);
    if (error) throw error;
  }

  if (inTable) {
    const { error } = await supabase.from(cfg.table).delete().eq("slug", slug);
    if (error) throw error;
  }

  console.log(`✓ Deleted ${slug}`);
}

/* -------------------------------------------------------------------------- */
/* List                                                                        */
/* -------------------------------------------------------------------------- */

async function listItems(cfg: ResolvedConfig) {
  const supabase = supabaseClient(cfg);

  const { data: files } = await supabase.storage.from(cfg.bucket).list();

  const storageSlugs = new Set(files?.map((f) => normalizeSlug(f.name)) ?? []);

  const { data: rows } = await supabase.from(cfg.table).select("slug");

  const tableSlugs = new Set(rows?.map((r) => normalizeSlug(r.slug)) ?? []);

  const all = new Set([...storageSlugs, ...tableSlugs]);

  if (all.size === 0) {
    console.log("No slugs found.");
    return;
  }

  console.log("slug".padEnd(32), "location");
  [...all].sort().forEach((slug) => {
    const loc =
      storageSlugs.has(slug) && tableSlugs.has(slug)
        ? "both"
        : storageSlugs.has(slug)
        ? "bucket"
        : "table";
    console.log(slug.padEnd(32), loc);
  });
}

/* -------------------------------------------------------------------------- */
/* Frontmatter                                                                 */
/* -------------------------------------------------------------------------- */

function parseFrontmatter(input: string): {
  frontmatter?: FrontMatter;
  body: string;
} {
  const trimmed = input.trimStart();
  if (!trimmed.startsWith("---")) {
    return { body: input };
  }

  const [, yamlBlock, rest] = trimmed.split(/---\s*/s);
  const fm = yaml.load(yamlBlock || "this is not good") as FrontMatter;

  return { frontmatter: fm, body: rest ?? "" };
}

/* -------------------------------------------------------------------------- */
/* Config                                                                      */
/* -------------------------------------------------------------------------- */

async function loadConfig(cliPath?: string): Promise<ResolvedConfig> {
  const cfg = cliPath ? await readConfig(cliPath) : {};

  const supabaseUrl = cfg.supabase_url ?? process.env.SUPABASE_URL;
  const serviceKey =
    cfg.supabase_service_key ?? process.env.SUPABASE_SERVICE_KEY;

  if (!supabaseUrl || !serviceKey) {
    throw new Error("Missing Supabase credentials");
  }

  return {
    supabaseUrl,
    serviceKey,
    bucket: cfg.bucket ?? "blog",
    table: cfg.table ?? "posts",
  };
}

async function readConfig(p: string): Promise<FileConfig> {
  const raw = await readFile(p, "utf8");
  return JSON.parse(JSON.stringify(require("toml").parse(raw)));
}

async function genConfig(): Promise<string> {
  const p = "./config.toml";
  await mkdir(path.dirname(p), { recursive: true });

  if (fs.existsSync(p)) {
    throw new Error("Config already exists");
  }

  await writeFile(
    p,
    `supabase_url = "https://xxxxx.supabase.co"
supabase_service_key = "service_role_key"
bucket = "blog"
`
  );

  return p;
}

/* -------------------------------------------------------------------------- */
/* Utils                                                                       */
/* -------------------------------------------------------------------------- */

function normalizeSlug(input: string): string {
  return path.basename(input, path.extname(input));
}

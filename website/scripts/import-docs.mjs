#!/usr/bin/env node
/**
 * Import ../docs/*.md into src/content/docs/ for Starlight.
 *
 * This script runs as a `prebuild` hook (see package.json scripts). It
 * copies the raw markdown from the monorepo's docs/ directory into the
 * Starlight content tree, inserts the YAML front matter Starlight needs
 * (title, description, sidebar order), and rewrites cross-doc links
 * from the docs/ layout (`[x](OTHER.md)`) to the Starlight layout
 * (`/section/slug/`).
 *
 * The src/content/docs/ tree is gitignored — this script is the single
 * source of truth for how docs get shaped for the website. Edit the
 * MAPPING table below to add/remove/rename pages.
 */

import { readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const WEBSITE_ROOT = resolve(__dirname, '..');
const MONOREPO_ROOT = resolve(WEBSITE_ROOT, '..');
const DOCS_SRC = join(MONOREPO_ROOT, 'docs');
const DOCS_DEST = join(WEBSITE_ROOT, 'src', 'content', 'docs');

/**
 * Mapping: source filename → { section, slug, sidebarOrder, title }.
 * Order here drives both the destination path and the sidebar position.
 * The `title` overrides the H1 (Starlight often wants shorter nav labels
 * than the H1 would give us).
 */
const MAPPING = [
	// Getting started
	{
		src: 'QUICKSTART.md',
		section: 'getting-started',
		slug: 'quickstart',
		order: 1,
		title: '5-Minute Quickstart',
	},
	{
		src: 'WINDOWS.md',
		section: 'getting-started',
		slug: 'windows',
		order: 2,
		title: 'Windows',
	},

	// How it works
	{
		src: 'ARCHITECTURE.md',
		section: 'how-it-works',
		slug: 'architecture',
		order: 1,
		title: 'Architecture',
	},
	{
		src: 'TOKEN_SAVINGS.md',
		section: 'how-it-works',
		slug: 'token-savings',
		order: 2,
		title: 'Token savings',
	},
	{
		src: 'MEMORY_MCP_DESIGN.md',
		section: 'how-it-works',
		slug: 'memory-mcp-design',
		order: 3,
		title: 'Memory MCP design',
	},
	{
		src: 'AGENTS.md',
		section: 'how-it-works',
		slug: 'agents',
		order: 4,
		title: 'Multi-agent systems',
	},
	{
		src: 'MEMORY_BRANCH_SCOPING.md',
		section: 'how-it-works',
		slug: 'memory-branch-scoping',
		order: 5,
		title: 'Memory branch scoping',
	},

	// Reference
	{
		src: 'CLI_REFERENCE.md',
		section: 'reference',
		slug: 'cli',
		order: 1,
		title: 'CLI reference',
	},
	{
		src: 'MCP_TOOLS.md',
		section: 'reference',
		slug: 'mcp-tools',
		order: 2,
		title: 'MCP tools',
	},
	{
		src: 'HTTP_API.md',
		section: 'reference',
		slug: 'http-api',
		order: 3,
		title: 'HTTP API',
	},

	// Integrations
	{
		src: 'INTEGRATIONS.md',
		section: 'integrations',
		slug: 'ai-coding-tools',
		order: 1,
		title: 'AI coding tools',
	},
	{
		src: 'OPENWEBUI.md',
		section: 'integrations',
		slug: 'open-webui',
		order: 2,
		title: 'Open WebUI',
	},

	// Operating
	{
		src: 'COOKBOOK.md',
		section: 'operating',
		slug: 'cookbook',
		order: 1,
		title: 'Cookbook',
	},
	{
		src: 'TROUBLESHOOTING.md',
		section: 'operating',
		slug: 'troubleshooting',
		order: 2,
		title: 'Troubleshooting',
	},
	{
		src: 'DATA_SAFETY.md',
		section: 'operating',
		slug: 'data-safety',
		order: 3,
		title: 'Data safety',
	},

	// Why CtxOne (strategy)
	{
		src: 'VISION.md',
		section: 'why-ctxone',
		slug: 'vision',
		order: 1,
		title: 'Vision',
	},
	{
		src: 'CONTEXT_ANXIETY.md',
		section: 'why-ctxone',
		slug: 'context-anxiety',
		order: 2,
		title: 'Context anxiety',
	},
	{
		src: 'TOKEN_ECONOMICS.md',
		section: 'why-ctxone',
		slug: 'token-economics',
		order: 3,
		title: 'Token economics',
	},
	{
		src: 'USE_CASES.md',
		section: 'why-ctxone',
		slug: 'use-cases',
		order: 4,
		title: 'Use cases',
	},
];

/** Build a lookup so we can rewrite cross-doc links. */
const FILENAME_TO_DEST = new Map();
for (const entry of MAPPING) {
	FILENAME_TO_DEST.set(entry.src, `/${entry.section}/${entry.slug}/`);
}

/**
 * Extract a one-sentence description from the first paragraph of body
 * text (skipping the H1 and any blank lines). Used for OG tags and
 * sidebar subtitles.
 */
function extractDescription(body) {
	const lines = body.split('\n');
	let i = 0;
	// Skip the first H1
	while (i < lines.length && !lines[i].startsWith('# ')) i++;
	i++; // past the H1
	// Skip blank lines
	while (i < lines.length && lines[i].trim() === '') i++;
	// Collect until the next blank line or heading
	const collected = [];
	while (
		i < lines.length &&
		lines[i].trim() !== '' &&
		!lines[i].startsWith('#')
	) {
		collected.push(lines[i]);
		i++;
	}
	const first = collected.join(' ').trim();
	// Trim to ~160 chars for clean OG tags
	if (first.length <= 160) return first;
	return first.slice(0, 157).trimEnd() + '…';
}

/**
 * Strip the H1 from the body (Starlight renders `title` from front
 * matter, so having the H1 in the body too would duplicate the heading).
 */
function stripH1(body) {
	const lines = body.split('\n');
	let i = 0;
	while (i < lines.length && !lines[i].startsWith('# ')) i++;
	if (i >= lines.length) return body;
	// Drop the H1 line and any blank lines that immediately follow it
	lines.splice(i, 1);
	while (i < lines.length && lines[i].trim() === '') {
		lines.splice(i, 1);
		break; // only strip one leading blank line
	}
	return lines.join('\n');
}

/**
 * Rewrite cross-doc markdown links.
 *
 * `[x](OTHER.md)`              → `[x](/section/slug/)`
 * `[x](OTHER.md#heading)`      → `[x](/section/slug/#heading)`
 * `[x](../docs/OTHER.md)`      → `[x](/section/slug/)`
 * `[x](docs/OTHER.md)`         → `[x](/section/slug/)`
 *
 * Links we don't know about (e.g. `[x](../LICENSE)`, absolute URLs,
 * relative paths into `examples/`) pass through unchanged — they will
 * 404 if clicked in the docs site, which is usually fine because the
 * target isn't part of the docs tree anyway.
 */
function rewriteLinks(body) {
	return body.replace(
		/\[([^\]]+)\]\(([^)]+)\)/g,
		(match, text, href) => {
			// Strip optional leading "./" or "../docs/" or "docs/"
			let normalized = href.trim();
			normalized = normalized.replace(/^\.\//, '');
			normalized = normalized.replace(/^\.\.\/docs\//, '');
			normalized = normalized.replace(/^docs\//, '');

			// Split off any #fragment
			const hashIdx = normalized.indexOf('#');
			const bare = hashIdx >= 0 ? normalized.slice(0, hashIdx) : normalized;
			const hash = hashIdx >= 0 ? normalized.slice(hashIdx) : '';

			const dest = FILENAME_TO_DEST.get(bare);
			if (!dest) return match;
			return `[${text}](${dest}${hash})`;
		}
	);
}

/**
 * YAML-escape a string for use in front matter. Minimal — just handles
 * the cases we actually hit (quotes, newlines). Since titles and
 * descriptions come from our own docs we can trust them to be
 * well-formed; we just quote and escape any embedded double quotes.
 */
function yamlString(s) {
	return `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

async function clean() {
	await rm(DOCS_DEST, { recursive: true, force: true });
	await mkdir(DOCS_DEST, { recursive: true });
}

async function writeIndex() {
	const indexPath = join(DOCS_DEST, 'index.mdx');
	const body = `---
title: CtxOne docs
description: Persistent, searchable, accountable memory for AI agents.
template: splash
hero:
  tagline: Memory that survives the session.
  actions:
    - text: Quickstart
      link: /getting-started/quickstart/
      icon: right-arrow
      variant: primary
    - text: View on GitHub
      link: https://github.com/ctxone/ctxone
      icon: external
---

import { Card, CardGrid } from '@astrojs/starlight/components';

<CardGrid>
  <Card title="New here?" icon="rocket">
    Start with the [Quickstart](/getting-started/quickstart/) — it gets you
    from zero to a running Hub and an AI tool that remembers things in
    under five minutes.
  </Card>

  <Card title="How it works" icon="puzzle">
    Read [Architecture](/how-it-works/architecture/) for the mental model,
    [Multi-agent systems](/how-it-works/agents/) for the system-centric
    architecture guide, and [Token savings](/how-it-works/token-savings/)
    for the context compensation argument.
  </Card>

  <Card title="Integrate" icon="setting">
    Wire CTXone into [AI coding tools](/integrations/ai-coding-tools/) with
    \`ctx init\`, or drop the [Open WebUI plugin](/integrations/open-webui/)
    into a self-hosted chat install.
  </Card>

  <Card title="Reference" icon="open-book">
    Full [CLI](/reference/cli/), [MCP tool](/reference/mcp-tools/), and
    [HTTP API](/reference/http-api/) references — every command, flag,
    endpoint, and response shape.
  </Card>
</CardGrid>
`;
	await writeFile(indexPath, body, 'utf8');
}

async function processOne(entry) {
	const srcPath = join(DOCS_SRC, entry.src);
	const destDir = join(DOCS_DEST, entry.section);
	const destPath = join(destDir, `${entry.slug}.md`);

	const raw = await readFile(srcPath, 'utf8');
	const description = extractDescription(raw);
	const withoutH1 = stripH1(raw);
	const linked = rewriteLinks(withoutH1);

	const frontMatter = [
		'---',
		`title: ${yamlString(entry.title)}`,
		`description: ${yamlString(description)}`,
		`sidebar:`,
		`  order: ${entry.order}`,
		'---',
		'',
	].join('\n');

	await mkdir(destDir, { recursive: true });
	await writeFile(destPath, frontMatter + linked, 'utf8');

	return { entry, bytes: raw.length };
}

async function main() {
	console.log(`[import-docs] source: ${DOCS_SRC}`);
	console.log(`[import-docs] dest:   ${DOCS_DEST}`);

	await clean();
	await writeIndex();

	let totalBytes = 0;
	for (const entry of MAPPING) {
		try {
			const { bytes } = await processOne(entry);
			totalBytes += bytes;
			console.log(
				`[import-docs] ${entry.src.padEnd(24)} → ${entry.section}/${entry.slug}`
			);
		} catch (err) {
			console.error(`[import-docs] ERROR processing ${entry.src}: ${err.message}`);
			process.exitCode = 1;
		}
	}
	console.log(
		`[import-docs] imported ${MAPPING.length} files (${(totalBytes / 1024).toFixed(1)} KiB)`
	);
}

main().catch((err) => {
	console.error('[import-docs] fatal:', err);
	process.exit(1);
});

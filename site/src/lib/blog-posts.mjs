// The blog's newest posts, for the home page's "From the blog" (web presence
// slice 8). The blog publishes them at /blog/posts.json (bynk-lang/blog,
// src/pages/posts.json.ts): newest first, each with its title, URL, summary,
// date, and the series, level and Bynk version from its front matter.
//
// The list is read once, at build time. If it can't be read (offline, the
// blog down, a malformed reply), the home page shows only its link to the
// blog and the build carries on with a warning: the home page never fails to
// build because of the blog.
//
// BYNK_BLOG_POSTS overrides where the list is read from: a URL, or a path to
// a local JSON file (for working on the home page offline).

import fs from "node:fs";

const DEFAULT = "https://bynk-lang.org/blog/posts.json";

/**
 * @typedef {{ title: string, url: string, summary?: string, date: string,
 *   series?: string, level?: string, bynkVersion?: string }} BlogPost
 */

/** @type {Promise<BlogPost[]> | undefined} */
let cached;

/** @returns {Promise<BlogPost[]>} */
async function load() {
  const source = process.env.BYNK_BLOG_POSTS || DEFAULT;
  try {
    const text = /^https?:\/\//.test(source)
      ? await fetch(source, { signal: AbortSignal.timeout(15_000) }).then((r) => {
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return r.text();
        })
      : fs.readFileSync(source, "utf8");
    const data = JSON.parse(text);
    const posts = Array.isArray(data) ? data : data.posts;
    if (!Array.isArray(posts)) throw new Error("no posts array");
    return posts.filter((p) => p && typeof p.title === "string" && typeof p.url === "string" && p.date);
  } catch (e) {
    console.warn(`[blog-posts] could not read ${source} (${e instanceof Error ? e.message : e}); "From the blog" shows only its link`);
    return [];
  }
}

/**
 * The newest `n` posts, newest first.
 * @param {number} n
 */
export async function latestPosts(n) {
  cached ??= load();
  const posts = await cached;
  return [...posts].sort((a, b) => Date.parse(b.date) - Date.parse(a.date)).slice(0, n);
}

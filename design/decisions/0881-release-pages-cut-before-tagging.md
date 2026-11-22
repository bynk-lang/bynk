# 0881 — Release notes are one page per release, cut from Unreleased before the release is tagged

- **Status:** Accepted (v0.640.14)

**Context.** The Book's changelog was one page: a table of every increment
since v0.5, newest first, written by the stamp (ADR 0206) one row per merge.
It had grown to 1,576 rows and 775 KB, the largest page on the site and more
than a fifth of the Book's words. Bynk already groups increments into
releases, cut at each track retirement before 1.0 and monthly after it, and
tagged by hand. But the changelog didn't show them, so a reader could not see
what a release contained without scrolling through one table. The web
presence track (#3861) settled one page per release tag (§14.3); what
remained was who writes which page, and when.

**Decision.** The Book keeps one page per release under
`reference/releases/`: `vX-Y-Z.md` for each tag (dashes, because Astro drops
the dots from a file name's URL, which would make `v0.63.91` and `v0.639.1`
collide), `unreleased.md` for the
increments merged since the latest tag, and an index listing every release,
newest first. The stamp writes each increment's row into Unreleased.
Cutting a release is a step before tagging: `cargo xtask release-page
--apply` turns Unreleased into the page for the workspace's current version,
starts an empty Unreleased, and adds the release to the index. The maintainer
merges that and tags the merge commit. `release.yml`'s `verify` job refuses a
tag whose page is missing, before anything is built or published. The
sidebar lists the release pages from the folder (`{release pages}` in
`SUMMARY.md`), so a cut needs no contents edit.

Rejected: having `release.yml` commit the rename to `main` after the tag is
pushed. That would give a read-only workflow write access to `main`, add a
second pusher beside the stamp, and leave a window in which the tagged commit
has no page. Rejected: grouping by minor version (about 640 pages, most of one
row). Rejected: generating the pages at build time from the tag list. The
site's build has no tags in its shallow clone, and the pages would stop being
plain Markdown that a reader of the repository can open.

**Consequences.** A release's page says what it shipped and when, and the
index reads as the project's release history. Cutting a release is one more
command, and forgetting it fails the release early instead of publishing a
release with no notes. A release cut adds a file and changes two; feature
PRs never touch the release pages, so they don't conflict with each other or
with a cut. A row's history moves when a release is cut, so a link to Unreleased
is a link to "since the latest release", not to a fixed list. `bynk book` and
`llms-full.txt` carry the same text as before, in more files. The pages move to
`/reference/releases/` when the track consolidates Reference.
Proposed in #3868.

---
level: patch
changelog: The docs site now wears the Bynk design system, from the same `@bynk-lang/site-theme` package as the blog — amber and teal in place of the indigo accent, Instrument Sans and JetBrains Mono, the bento-blocks wordmark, and the blog's header — in light and dark, following the reader's setting (web presence track, slice 6, #4006).
---

## ADR: one-site-theme-package
title: The docs site and the blog share one theme package, @bynk-lang/site-theme, published to GitHub Packages
summary: The Bynk brand for Starlight sites lives in bynk-lang/site-theme as a Starlight plugin both sites depend on; a site's own CSS and component overrides win over the theme's

**Context.** The docs site and the blog looked like two projects. The docs
site carried the brand from the documentation track (an electric-indigo
accent, Inter and Literata, a plain-text wordmark, its own copy of Starlight's
header); the blog used the design system's brand (amber and teal, Instrument
Sans and JetBrains Mono, the bento-blocks mark). Each brand change had to be
made twice, and the two already disagreed. The web presence track (#3861, §8)
planned one theme from one source, first at `site/theme/` in this repository.

**Decision.** The brand is one package, `@bynk-lang/site-theme`, in its own
repository ([bynk-lang/site-theme](https://github.com/bynk-lang/site-theme)),
published to GitHub Packages under the `bynk-lang` organisation. It is a
Starlight plugin carrying the design system's tokens as `--bynk-*` custom
properties (dark and light), Starlight's variables mapped onto them, the
`Head` (fonts), `Header` (wordmark and cross-site navigation) and `SiteTitle`
components, the Bynk TextMate grammar, and dark and light code themes. Both
sites depend on a released version. A site's own `customCss` comes after the
theme's, a component the site overrides stays the site's, and a `bynk` grammar
the site registers wins over the theme's. The docs site keeps its `Sidebar`
(scoped to the Book, By Example or the Reference), a small `site.css` for its
landing hero, and its own copy of the grammar, which the `textmate_keywords`
test guards; CI fails if that copy differs from the theme's.

**Consequences.** A design-system change reaches both sites through one
theme release and a version bump in each. The package lives outside this
repository so it can ship on its own cadence. GitHub Packages needs a token
even to read: CI and the deploy read it with the workflow token
(`packages: read`), Dependabot with a `read:packages` secret, and a local
build of the site with a `read:packages` token in the contributor's own
`~/.npmrc`. If the site gains outside contributors, publishing the theme to
the public npm registry removes that step; that is a theme decision.

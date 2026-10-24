---
title: "Serve a page from an HTML file"
---
**Goal:** keep a page's markup in its own `.html` file, fill in the parts that
change, and serve it from an HTTP handler.

A page built from string literals gets hard to read quickly: markup, escaping
and logic share one line. An [`embed`](/book/reference/embeds/) reads a file
into a `String`, so the static markup stays a real HTML file and Bynk only
fills in the gaps.

## The page shell

Put the HTML in `embeds/page.html`, with a marker wherever something is
filled in:

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{{title}}</title>
  <link rel="stylesheet" href="{{style}}">
</head>
<body>
  <h1>{{title}}</h1>
  {{content}}
</body>
</html>
```

The `{{ }}` markers are only a convention of this file; the language gives
them no meaning.

## Fill it in

Declare the embed, and the stylesheet as an [asset](/book/reference/assets/)
so the page links a real, content-hashed URL:

```bynk,fragment
commons app.page

asset style = "assets/page.css"

embed shell = "embeds/page.html"

---
Escapes text for an HTML body or a double-quoted attribute.
---
fn escape(s: String) -> String {
  s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("\"", "&quot;")
}

fn render(title: String, content: String) -> String {
  shell
    .replace("{{style}}", style.url)
    .replace("{{title}}", escape(title))
    .replace("{{content}}", content)
}
```

`content` is markup you built (a table, say), so it is not escaped here; the
caller escaped the text inside it. Escaping is yours to do: an embed hands you
text and nothing more.

## Serve it

A handler returns the page as a `Raw` result, with the page's content type:

```bynk,fragment
context app.web

uses app.page

service web from http {
  on GET("/") () -> Effect[HttpResult[()]] by Visitor {
    Raw(Bytes.fromUtf8(render("Home", "<p>Hello</p>")), "text/html; charset=utf-8")
  }
}
```

## Test it

A unit suite can name the embed, so a test can assert that every placeholder
was filled:

```bynk,fragment
suite app.web {
  case "the page fills every placeholder" {
    let page = render("Home", "<p>Hello</p>")
    expect page.contains("<title>Home</title>")
    expect page.contains("{{") == false
  }
}
```

**See also:** [Reference — Embedded text](/book/reference/embeds/),
[Static assets](/book/reference/assets/).

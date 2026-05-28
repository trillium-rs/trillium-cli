---
title: HTML rewriting
slug: /gateway/rewrite-html
---

# `rewrite-html`

The `rewrite-html` directive streams a route's HTML response through
[lol-html](https://docs.rs/lol-html), applying ordered mutations to elements
matched by CSS selectors. Use it to inject tags, rewrite attributes, or strip
nodes from a static page or a proxied upstream — without buffering the whole
body.

```kdl
route "/*" {
    proxy {
        upstream "http://127.0.0.1:9000"
    }

    rewrite-html {
        select "head" {
            append "<script src=\"/analytics.js\" async></script>"
        }
        select "a[target=_blank]" {
            set-attribute "rel" "noopener noreferrer"
        }
        select "img" {
            set-attribute "loading" "lazy"
        }
        select ".legacy-banner" {
            remove
        }
        select "title" {
            set-text "Proxied by trillium"
        }
    }
}
```

## Only HTML is touched

The rewriter self-gates on the response `Content-Type`: only responses whose
subtype is `html` are transformed. JSON, images, and other binary bodies stream
through untouched. That makes `rewrite-html` safe to drop in front of a mixed
[`proxy`](./routing#proxy) that serves both pages and an API.

Because it transforms the body produced by the preceding directive, place
`rewrite-html` **after** the body-producing directive (`proxy` or `files`) in
the route.

## Selectors are validated at load time

Each `select` takes a CSS selector. lol-html supports a subset of CSS
selectors, and they're checked when the config loads (or under `--check`) — not
on the first matching request. An unsupported or malformed selector fails
immediately with a [`miette`](https://docs.rs/miette) span pointing at the
offending string.

## Element operations

Each `select "css-selector"` block holds an ordered list of mutations applied to
every matching element. Markup-valued operations insert their argument as HTML;
`set-text` inserts HTML-escaped text.

| Operation                   | Effect                                                       |
|-----------------------------|--------------------------------------------------------------|
| `before "<markup>"`         | insert markup immediately before the element                 |
| `after "<markup>"`          | insert markup immediately after the element                  |
| `prepend "<markup>"`        | insert markup as the element's first child                   |
| `append "<markup>"`         | insert markup as the element's last child                    |
| `set-inner "<markup>"`      | replace the element's inner content with markup              |
| `replace "<markup>"`        | replace the element and its content with markup              |
| `set-text "text"`           | replace inner content with HTML-escaped text                 |
| `set-attribute "name" "val"`| set (or replace) an attribute                                |
| `remove-attribute "name"`   | remove an attribute                                          |
| `set-tag "div"`             | rename the element's tag                                     |
| `remove`                    | delete the element and its content                           |
| `unwrap`                    | drop the element's tags but keep its inner content           |

## Examples

Inject a script into every page from a static site:

```kdl
route "/*" {
    files root="./public"
    rewrite-html {
        select "body" {
            append "<script src=\"/live-reload.js\"></script>"
        }
    }
}
```

Harden outbound links and lazy-load images on a proxied upstream:

```kdl
route "/*" {
    proxy {
        upstream "http://legacy-app:9000"
    }
    rewrite-html {
        select "a[target=_blank]" {
            set-attribute "rel" "noopener noreferrer"
        }
        select "img" {
            set-attribute "loading" "lazy"
        }
    }
}
```

Strip a node and rewrite the title:

```kdl
route "/*" {
    files root="./public"
    rewrite-html {
        select ".tracking-pixel" {
            remove
        }
        select "title" {
            set-text "My Site"
        }
    }
}
```

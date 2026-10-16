# css-class-lsp

A small language server (and Neovim plugin) for **your own CSS classes**: the ones
defined in the project's stylesheets, not Tailwind utilities. It's meant to run
next to the Tailwind CSS language server.

- **Completion** inside class strings, with the source file in the menu and the
  CSS rule plus the reference count in the docs preview
- **Go to definition / declaration**: jumps to the selector in the stylesheet
- **Find references**: every use in source files (and `@apply` in CSS)
- **Hover**: definition location(s), reference count, and the CSS rule
- **Diagnostics** for classes that aren't defined anywhere (configurable severity,
  can be turned off), and `:CssClassQuickfix` to list every one in the workspace
- **Workspace symbols**: `.class` entries for pickers (Telescope / snacks / fzf-lua)

It doesn't fully parse CSS. A forgiving scanner finds class selectors in rule
preludes, and handles nesting (`@layer`, `@media`, SCSS `&__elem`), escapes
(`.md\:flex`), and Tailwind v4 `@utility name {}`. It also reads `<style>` blocks
in HTML/Vue/Svelte/Astro.

## Install

Requires Neovim 0.11+ and a Rust toolchain (the server is built on install).

### lazy.nvim

```lua
{
  "MasterTemple/css-class-completion",
  build = "cargo build --release",
  opts = {},
  config = function(_, opts) require("css-class-lsp").setup(opts) end,
}
```

### Other plugin managers

Install `MasterTemple/css-class-completion`, run `cargo build --release` in the
plugin directory, and call `require("css-class-lsp").setup({})`.

The plugin starts `target/release/css-class-lsp` from its own directory if it
exists, and otherwise `css-class-lsp` on `$PATH`. To install the server
separately:

```sh
cargo install --git https://github.com/MasterTemple/css-class-completion
```

### Telescope (optional)

```lua
require("telescope").load_extension("css_class")
```

- `:Telescope css_class` or `:Telescope css_class undefined`: search undefined
  classes across the whole workspace
- `:Telescope css_class classes`: search every defined class and jump to it

## Where classes are recognised

Built-in patterns (each capture group is a space separated class list):

| Pattern | Example |
| --- | --- |
| `class`/`className` `=` or `:` + string | `class="a b"`, `class: "a b"` (RSX), `className='a'` |
| JSX braces | ``className={`a b`}`` |
| Dioxus conditionals | `class: if on { "a" } else { "b" }` |
| Dioxus `format!` | `class: format!("a {x}")` |
| clsx-style calls | `clsx("a", c && "b")`, `cn(...)`, `twMerge(...)` |

In Rust format strings, `{expr}` parts are skipped. For anything else, add
regexes. The syntax is the same as Tailwind's `experimental.classRegex`, so you
can reuse yours:

```lua
require("css-class-lsp").setup({
  settings = {
    class_regex = {
      [[class:[^"]*"(.*)"]],                         -- single regex
      { [[cva\(([^)]*)\)]], [=["([^"]*)"]=] },       -- {container, inner}
    },
    exclude = { "assets/vendor/**" },
  },
})
```

## Options

```lua
{
  cmd = nil,                    -- server command (auto-detected)
  filetypes = { ... },
  root_markers = { ".git", "Dioxus.toml", "Cargo.toml", "package.json" },
  watch_files = "auto",         -- didChangeWatchedFiles; "auto" = only if inotifywait exists on Linux
  reindex_on_focus = true,      -- rescan on FocusGained (catches git checkouts, etc.)
  settings = {                  -- sent as init_options
    class_regex = {},
    disable_default_patterns = false,
    exclude = {},               -- globs, absolute or relative to the root
    css_extensions = { "css", "scss", "less", "pcss", "postcss" },
    source_extensions = { "rs", "html", "htm", "jsx", "tsx", "js", "ts", "vue", "svelte", "astro", ... },
    skip_generated = true,      -- ignore *.min.css and Tailwind build output ("/*! tailwindcss v...")
    respect_gitignore = true,
    max_file_size = 2097152,
    tailwind_output = {},       -- extra paths of Tailwind build output (see below)
    diagnostics = {
      enabled = true,
      severity = "warning",     -- "error" | "warning" | "info" | "hint" (or vim.diagnostic.severity.*)
      ignore = {},              -- regexes; matching class names are never reported
    },
  },
}
```

## Diagnostics and Tailwind

A class used in a source file but not defined in any stylesheet gets a
diagnostic (`undefined-class`). Only open buffers get live diagnostics. Run
`:CssClassQuickfix` to put every undefined class in the workspace into the
quickfix list.

To avoid flagging every Tailwind utility, the server reads Tailwind's build
output. Classes in it count as defined, but they don't show up in completion,
hover or definitions, because the Tailwind server already covers those. The
output is found by its `/*! tailwindcss v...` banner. These locations are read
even when gitignored: `assets/tailwind.css` (Dioxus's default),
`public/tailwind.css`, `static/tailwind.css` and `assets/output.css`. For any
other location, add it to `tailwind_output`. The server checks these files
every 750ms, so a new utility stops being flagged as soon as `dx serve`
rebuilds the CSS.

If there's no build output, add patterns to `diagnostics.ignore` or set
`diagnostics.enabled = false`. Tokens next to interpolation (`text-{size}`) and
bare identifiers in object syntax (`{ active: cond }`) are never reported.

`skip_generated` matters for Dioxus. Without it, the generated
`assets/tailwind.css` would add every utility you use to completion, duplicating
what the Tailwind server already gives you.

Commands: `:CssClassReindex`, `:CssClassInfo` (shows indexed stylesheets) and
`:CssClassQuickfix`. You can also call these from Lua:
`require("css-class-lsp").reindex()`, `.info()` and `.quickfix()`.

## Development

```sh
cargo test
cargo build --release
```

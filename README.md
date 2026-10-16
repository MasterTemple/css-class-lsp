# css-class-lsp

A small language server (and Neovim plugin) for **your own CSS classes**: the ones
defined in the project's stylesheets, not Tailwind utilities. It's meant to run
next to the Tailwind CSS language server.

- **Completion** inside class strings, with the source file in the menu and the
  CSS rule plus the reference count in the docs preview
- **Go to definition**: jumps to the selector in the stylesheet
- **Find references**: every use in source files (and `@apply` in CSS)
- **Hover**: definition location(s), reference count, and the CSS rule
- **Workspace symbols**: `.class` entries for pickers (Telescope / snacks / fzf-lua)

It doesn't fully parse CSS. A forgiving scanner finds class selectors in rule
preludes, and handles nesting (`@layer`, `@media`, SCSS `&__elem`), escapes
(`.md\:flex`), and Tailwind v4 `@utility name {}`. It also reads `<style>` blocks
in HTML/Vue/Svelte/Astro.

## Install (lazy.nvim)

```lua
{
  dir = "~/Dropbox/Development/rust/css-class-completion", -- or the git URL
  build = "cargo build --release",
  ft = { "rust", "html", "css", "scss", "javascriptreact", "typescriptreact", "vue", "svelte" },
  opts = {},
  config = function(_, opts) require("css-class-lsp").setup(opts) end,
}
```

The plugin uses `target/release/css-class-lsp` inside the plugin directory, or
`css-class-lsp` on `$PATH` (`cargo install --path .`). It requires Neovim 0.11 or
newer (`vim.lsp.config`); older versions fall back to `vim.lsp.start`.

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
  },
}
```

`skip_generated` matters for Dioxus. Without it, the generated
`assets/tailwind.css` would add every utility you use to completion, duplicating
what the Tailwind server already gives you.

Commands: `:CssClassReindex` and `:CssClassInfo` (shows indexed stylesheets).

## Development

```sh
cargo test
cargo build --release
```

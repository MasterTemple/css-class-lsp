local M = {}

M.name = "css_class_lsp"

M.defaults = {
	-- Command to start the server. Defaults to the binary built inside this
	-- plugin (`cargo build --release`), then `css-class-lsp` on $PATH.
	cmd = nil,
	filetypes = {
		"html",
		"css",
		"scss",
		"less",
		"rust",
		"javascript",
		"javascriptreact",
		"typescript",
		"typescriptreact",
		"vue",
		"svelte",
		"astro",
	},
	root_markers = { ".git", "Dioxus.toml", "Cargo.toml", "package.json" },
	-- Watch files changed outside Neovim (git checkout, codegen, ...).
	-- "auto" enables it only when `inotifywait` is available (cheap on Linux).
	watch_files = "auto",
	-- Re-index when Neovim regains focus.
	reindex_on_focus = true,
	-- Passed to the server as `init_options`.
	settings = {
		-- Extra regexes, same idea as Tailwind's `experimental.classRegex`.
		-- Each capture group is a space separated class list. A {container, inner}
		-- pair runs `inner` inside group 1 of `container`.
		class_regex = {},
		-- Set to true to only use `class_regex`.
		disable_default_patterns = false,
		-- Globs (absolute or relative to the root) to skip.
		exclude = {},
		-- css_extensions = { "css", "scss", "less", "pcss", "postcss" },
		-- source_extensions = { "rs", "html", "jsx", "tsx", ... },
		-- Skip `*.min.css` and Tailwind build output (`/*! tailwindcss v...`).
		skip_generated = true,
		respect_gitignore = true,
		max_file_size = 2 * 1024 * 1024,
	},
}

local function plugin_root()
	local src = debug.getinfo(1, "S").source:sub(2)
	return vim.fn.fnamemodify(src, ":p:h:h:h")
end

local function find_cmd()
	local root = plugin_root()
	for _, p in ipairs({ "/target/release/css-class-lsp", "/target/debug/css-class-lsp" }) do
		if vim.uv.fs_stat(root .. p) then
			return { root .. p }
		end
	end
	if vim.fn.executable("css-class-lsp") == 1 then
		return { "css-class-lsp" }
	end
end

local function clients(bufnr)
	return vim.lsp.get_clients({ name = M.name, bufnr = bufnr })
end

function M.reindex()
	for _, c in ipairs(clients()) do
		c:request("cssClassLsp/reindex", vim.empty_dict(), function() end)
	end
end

function M.info()
	local cs = clients()
	if #cs == 0 then
		vim.notify("css-class-lsp: no active client", vim.log.levels.WARN)
		return
	end
	for _, c in ipairs(cs) do
		c:request("cssClassLsp/stats", vim.empty_dict(), function(err, res)
			if err or not res then
				vim.notify("css-class-lsp: " .. vim.inspect(err), vim.log.levels.ERROR)
				return
			end
			local lines = {
				("css-class-lsp: %d classes, %d files indexed%s"):format(
					res.classes,
					res.files,
					res.indexing and " (indexing…)" or ""
				),
				"roots: " .. table.concat(res.roots, ", "),
				"stylesheets:",
			}
			for _, s in ipairs(res.stylesheets) do
				table.insert(lines, "  " .. s)
			end
			vim.notify(table.concat(lines, "\n"))
		end)
	end
end

function M.setup(opts)
	opts = vim.tbl_deep_extend("force", M.defaults, opts or {})
	local cmd = opts.cmd or find_cmd()
	if not cmd then
		vim.notify(
			"css-class-lsp: server binary not found. Run `cargo build --release` in "
				.. plugin_root()
				.. " or put css-class-lsp on $PATH.",
			vim.log.levels.WARN
		)
		return
	end

	local watch = opts.watch_files
	if watch == "auto" then
		watch = vim.fn.has("linux") == 0 or vim.fn.executable("inotifywait") == 1
	end

	local config = {
		cmd = cmd,
		filetypes = opts.filetypes,
		root_markers = opts.root_markers,
		init_options = opts.settings,
		capabilities = {
			workspace = { didChangeWatchedFiles = { dynamicRegistration = watch and true or false } },
		},
	}

	if vim.lsp.config and vim.lsp.enable then
		vim.lsp.config(M.name, config)
		vim.lsp.enable(M.name)
	else
		-- Neovim < 0.11
		vim.api.nvim_create_autocmd("FileType", {
			group = vim.api.nvim_create_augroup("css_class_lsp_start", { clear = true }),
			pattern = opts.filetypes,
			callback = function(ev)
				local root = vim.fs.root(ev.buf, opts.root_markers)
				vim.lsp.start(vim.tbl_extend("force", config, {
					name = M.name,
					root_dir = root,
					capabilities = vim.tbl_deep_extend(
						"force",
						vim.lsp.protocol.make_client_capabilities(),
						config.capabilities
					),
				}), { bufnr = ev.buf })
			end,
		})
	end

	vim.api.nvim_create_user_command("CssClassReindex", M.reindex, { desc = "Re-scan CSS classes in the workspace" })
	vim.api.nvim_create_user_command("CssClassInfo", M.info, { desc = "Show css-class-lsp index stats" })

	if opts.reindex_on_focus then
		local last = 0
		vim.api.nvim_create_autocmd("FocusGained", {
			group = vim.api.nvim_create_augroup("css_class_lsp_focus", { clear = true }),
			callback = function()
				local now = vim.uv.now()
				if now - last > 5000 then
					last = now
					M.reindex()
				end
			end,
		})
	end
end

return M

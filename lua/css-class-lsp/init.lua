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
		-- Paths (relative to the root) of Tailwind build output. Classes in it
		-- count as defined for diagnostics but stay out of completion/hover.
		-- Common locations like assets/tailwind.css are detected automatically.
		tailwind_output = {},
		diagnostics = {
			-- Report classes that aren't defined in any stylesheet.
			enabled = true,
			-- "error" | "warning" | "info" | "hint", or vim.diagnostic.severity.*
			severity = "warning",
			-- Regexes (Rust syntax); matching class names are never reported.
			ignore = {},
		},
	},
}

local function plugin_root()
	local src = debug.getinfo(1, "S").source:sub(2)
	return vim.fn.fnamemodify(src, ":p:h:h:h")
end

-- Where `cargo build` may have put the binary besides <plugin>/target:
-- CARGO_TARGET_DIR or `build.target-dir` in a cargo config.
local function other_target_dirs(root)
	local dirs = {}
	if vim.env.CARGO_TARGET_DIR then
		table.insert(dirs, (vim.fn.fnamemodify(vim.env.CARGO_TARGET_DIR, ":p"):gsub("/$", "")))
	end
	if vim.fn.executable("cargo") == 1 then
		local out = vim.system({ "cargo", "metadata", "--format-version", "1", "--no-deps", "--offline" }, { cwd = root })
			:wait(5000)
		local ok, meta = pcall(vim.json.decode, out.stdout or "")
		if out.code == 0 and ok and type(meta) == "table" and meta.target_directory then
			table.insert(dirs, meta.target_directory)
		end
	end
	return dirs
end

local bin = vim.fn.has("win32") == 1 and "css-class-lsp.exe" or "css-class-lsp"

local function find_in(dirs, searched)
	for _, dir in ipairs(dirs) do
		for _, p in ipairs({ "/release/" .. bin, "/debug/" .. bin }) do
			if vim.list_contains(searched, dir .. p) then
				break
			end
			table.insert(searched, dir .. p)
			if vim.uv.fs_stat(dir .. p) then
				return { dir .. p }
			end
		end
	end
end

--- Returns the server command, or nil and the paths that were searched.
local function find_cmd()
	local root = plugin_root()
	local searched = {}
	local cmd = find_in({ root .. "/target" }, searched)
	if cmd then
		return cmd
	end
	if vim.fn.executable("css-class-lsp") == 1 then
		return { "css-class-lsp" }
	end
	return find_in(other_target_dirs(root), searched), searched
end

local function clients(bufnr)
	return vim.lsp.get_clients({ name = M.name, bufnr = bufnr })
end

-- Why there's no client, for the "no active client" message.
local setup_error, last_exit

local function no_client()
	local msg = "css-class-lsp: no active client"
	if setup_error then
		msg = msg .. "\n" .. setup_error
	elseif last_exit then
		msg = msg .. ("\nserver exited (code %d, signal %d); see :LspLog"):format(last_exit.code, last_exit.signal)
	elseif not M.config then
		msg = msg .. "\nsetup() was never called"
	else
		msg = msg
			.. ("\nnot attached to this buffer (filetype %q); root markers: %s"):format(
				vim.bo.filetype,
				table.concat(M.config.root_markers, ", ")
			)
	end
	vim.notify(msg, vim.log.levels.WARN)
end

function M.reindex()
	for _, c in ipairs(clients()) do
		c:request("cssClassLsp/reindex", vim.empty_dict(), function() end)
	end
end

function M.info()
	local cs = clients()
	if #cs == 0 then
		no_client()
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

local qf_type = { "E", "W", "I", "N" }

--- Collect every undefined class in the workspace as quickfix items and pass
--- them to `on_done(items)`. Reports errors itself; `on_done` is not called
--- when no client is running or the index isn't ready.
function M.undefined_items(on_done)
	local cs = clients()
	if #cs == 0 then
		no_client()
		return
	end
	local items, pending, ready = {}, #cs, true
	for _, c in ipairs(cs) do
		c:request("cssClassLsp/undefinedClasses", vim.empty_dict(), function(err, res)
			if err then
				vim.notify("css-class-lsp: " .. vim.inspect(err), vim.log.levels.ERROR)
			elseif res == vim.NIL or res == nil then
				ready = false
			else
				for _, d in ipairs(res) do
					table.insert(items, {
						filename = d.filename,
						lnum = d.lnum,
						col = d.col,
						end_lnum = d.lnum,
						end_col = d.end_col,
						type = qf_type[d.severity] or "W",
						text = ("CSS class `%s` is not defined in any stylesheet"):format(d.name),
						user_data = { class = d.name },
					})
				end
			end
			pending = pending - 1
			if pending == 0 then
				if not ready and #items == 0 then
					vim.notify("css-class-lsp: still indexing, try again in a moment", vim.log.levels.WARN)
					return
				end
				on_done(items)
			end
		end)
	end
end

--- Put every undefined class in the workspace into the quickfix list.
function M.quickfix()
	M.undefined_items(function(items)
		vim.fn.setqflist({}, " ", { title = "Undefined CSS classes", items = items })
		if #items == 0 then
			vim.notify("css-class-lsp: no undefined classes")
		else
			vim.cmd("botright copen")
		end
	end)
end

--- All defined classes as quickfix-style items (via workspace/symbol).
function M.class_items(on_done)
	local cs = clients()
	if #cs == 0 then
		no_client()
		return
	end
	local items, pending = {}, #cs
	for _, c in ipairs(cs) do
		c:request("workspace/symbol", { query = "" }, function(err, res)
			if not err and res then
				vim.list_extend(items, vim.lsp.util.symbols_to_items(res, 0, c.offset_encoding))
			end
			pending = pending - 1
			if pending == 0 then
				on_done(items)
			end
		end)
	end
end

function M.setup(opts)
	opts = vim.tbl_deep_extend("force", M.defaults, opts or {})
	M.config = opts
	setup_error, last_exit = nil, nil
	local cmd, searched = opts.cmd, nil
	if not cmd then
		cmd, searched = find_cmd()
	end
	if not cmd then
		setup_error = "server binary not found. Run `cargo build --release` in "
			.. plugin_root()
			.. " or put css-class-lsp on $PATH. Searched:\n  "
			.. table.concat(searched, "\n  ")
		vim.notify("css-class-lsp: " .. setup_error, vim.log.levels.WARN)
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
		on_exit = function(code, signal)
			if code ~= 0 or signal ~= 0 then
				last_exit = { code = code, signal = signal }
			end
		end,
	}

	if not cmd then
		-- Keep the commands below so they can report why there's no client.
	elseif vim.lsp.config and vim.lsp.enable then
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
	vim.api.nvim_create_user_command(
		"CssClassQuickfix",
		M.quickfix,
		{ desc = "Quickfix list of undefined CSS classes in the workspace" }
	)

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
